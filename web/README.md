# Attune Web UI

Modern React-based web interface for the Attune automation platform.

## Overview

The Attune Web UI is a single-page application (SPA) built with React 18, TypeScript, and Vite that provides a comprehensive interface for managing and monitoring the Attune automation platform.

## Tech Stack

- **React 18** - UI framework
- **TypeScript 5** - Type safety
- **Vite** - Build tool and dev server
- **React Router v6** - Client-side routing
- **TanStack Query (React Query v5)** - Server state management
- **Axios** - HTTP client
- **Tailwind CSS v3** - Styling
- **Zustand** - Client state management (minimal usage)

## Prerequisites

- Node.js 18+ and npm
- Attune API service running on `http://localhost:8080` (or configured URL)

## Getting Started

### Installation

```bash
cd web
npm install
```

### Development

```bash
# Start development server (runs on http://localhost:3000)
npm run dev

# Build for production
npm run build

# Preview production build
npm run preview

# Lint code
npm run lint
```

### Environment Configuration

Create a `.env.development` file (already provided) or customize:

```env
VITE_API_BASE_URL=http://localhost:8080
VITE_WS_URL=ws://localhost:8081
VITE_APP_NAME=Attune
VITE_APP_VERSION=0.1.0
```

## Project Structure

```
web/
├── src/
│   ├── api/                  # OpenAPI generated code (run generate:api)
│   ├── components/
│   │   ├── common/          # Shared components
│   │   ├── layout/          # Layout components (MainLayout, etc.)
│   │   └── ui/              # UI primitives
│   ├── contexts/            # React contexts (Auth, etc.)
│   ├── hooks/               # Custom React hooks
│   ├── lib/                 # Library setup (API client, query client)
│   ├── pages/               # Page components
│   │   ├── auth/           # Login page
│   │   ├── dashboard/      # Dashboard
│   │   ├── packs/          # Pack management (TODO)
│   │   ├── actions/        # Action management (TODO)
│   │   ├── rules/          # Rule management (TODO)
│   │   └── executions/     # Execution monitoring (TODO)
│   ├── types/               # TypeScript type definitions
│   ├── utils/               # Utility functions
│   ├── App.tsx              # Root component with routing
│   ├── main.tsx             # Application entry point
│   └── index.css            # Global styles
├── public/                  # Static assets
├── .env.development         # Development environment variables
├── .env.example             # Environment variables template
├── package.json
├── tsconfig.json            # TypeScript configuration
├── vite.config.ts           # Vite configuration
└── tailwind.config.js       # Tailwind CSS configuration
```

## Features

### ✅ Implemented

- **Authentication**: JWT-based login with token refresh
- **Protected Routes**: Automatic redirect to login for unauthenticated users
- **Main Layout**: Sidebar navigation with user profile
- **Dashboard**: Basic dashboard with placeholder stats
- **API Client**: Axios instance with request/response interceptors
- **Type Safety**: Full TypeScript coverage with shared types

### 🚧 In Progress

- API client code generation from OpenAPI spec
- Real-time updates via WebSocket
- TanStack Query hooks for data fetching

### 📋 TODO

- Pack browser and management
- Action list and editor
- Rule list and editor
- Execution history and monitoring
- Event stream viewer
- Workflow visual editor
- User management
- Settings page

## API Client Generation ⭐

**The web UI uses an auto-generated TypeScript client from the backend's OpenAPI specification.**

This ensures type safety, schema validation, and automatic synchronization with the backend API.

### Generate/Update Client

```bash
# Ensure API service is running first
npm run generate:api
```

This will:

1. Download the OpenAPI spec from `http://localhost:8080/api-spec/openapi.json`
2. Generate TypeScript types in `src/api/models/` (~90 files)
3. Generate API service classes in `src/api/services/` (13 services)
4. Generate Axios client configuration in `src/api/core/`

### Usage

**✅ Use generated services (type-safe):**

```typescript
import { PacksService, AuthService } from "@/api";
import type { CreatePackRequest } from "@/api";

// Login
const response = await AuthService.login({
  requestBody: { login: "admin", password: "secret" },
});

// List packs with full type safety
const packs = await PacksService.listPacks({ page: 1, pageSize: 50 });

// Create pack - TypeScript validates schema!
const pack = await PacksService.createPack({
  requestBody: {
    ref: "my-pack",
    label: "My Pack",
    description: "Custom pack",
  },
});
```

**❌ Don't use manual axios calls:**

```typescript
// NO - this has no type safety and can easily break
await apiClient.post("/api/v1/packs", { name: "wrong-field" });
```

### Benefits

- ✅ **Full TypeScript types** - All requests/responses typed
- ✅ **Compile-time validation** - Catch schema mismatches before runtime
- ✅ **Auto-completion** - IDE support for all API methods
- ✅ **Always in sync** - Regenerate when backend changes
- ✅ **Less code** - No manual type definitions needed

### Available Services

- `AuthService` - Login, register, token refresh
- `PacksService` - Pack CRUD operations
- `ActionsService` - Action management
- `RulesService` - Rule configuration
- `ExecutionsService` - Execution tracking
- `HealthService` - Health checks
- And 7 more...

### Documentation

- **Generated API Docs:** `src/api/README.md`
- **Migration Guide:** `MIGRATION-TO-GENERATED-CLIENT.md`
- **Backend Docs:** `../docs/openapi-client-generation.md`
- **Interactive Swagger UI:** http://localhost:8080/docs

## Authentication Flow

1. User enters credentials on `/login`
2. App sends POST to `/auth/login`
3. Server returns `access_token` and `refresh_token`
4. Tokens stored in localStorage
5. API client automatically adds `Authorization: Bearer <token>` to requests
6. On 401 response, client attempts token refresh
7. If refresh fails, user redirected to login

## Development Guidelines

### Component Structure

- Use functional components with hooks
- Keep components small and focused
- Use TypeScript for all components
- Prefer composition over prop drilling

### State Management

- **Server State**: Use TanStack Query (React Query)
- **Client State**: Use React Context or Zustand
- **URL State**: Use React Router params/search

### Styling

- Use Tailwind CSS utility classes
- Follow mobile-first responsive design
- Use consistent spacing (4, 8, 16, 24, 32px)
- Prefer semantic color names from Tailwind

### API Integration

**Use generated services with React Query:**

```typescript
import { useQuery, useMutation } from "@tanstack/react-query";
import { ActionsService } from "@/api";
import type { CreateActionRequest } from "@/api";

// Fetch data with full type safety
const { data, isLoading } = useQuery({
  queryKey: ["actions"],
  queryFn: () => ActionsService.listActions({ page: 1, pageSize: 50 }),
});

// Mutate data with schema validation
const mutation = useMutation({
  mutationFn: (data: CreateActionRequest) =>
    ActionsService.createAction({ requestBody: data }),
  onSuccess: () => {
    queryClient.invalidateQueries({ queryKey: ["actions"] });
  },
});
```

## Building for Production

```bash
npm run build
```

Output is in `dist/` directory. Serve with any static file server:

```bash
# Preview locally
npm run preview

# Or use any static server
npx serve -s dist
```

## Troubleshooting

### API Client Out of Sync

**If the backend API changes, regenerate the client:**

```bash
# Start API server first
cd ../crates/api && cargo run --bin attune-api

# In another terminal, regenerate client
cd web
npm run generate:api
```

**Fix any TypeScript errors** that appear after regeneration - these indicate breaking API changes.

### CORS Issues

Ensure the API service has CORS enabled for `http://localhost:3000` in development.

### Build Errors

Clear the TypeScript build cache:

```bash
rm -rf node_modules/.tmp
npm run build
```

## Contributing

1. Keep the architecture document up to date: `../docs/web-ui-architecture.md`
2. Add new pages to the router in `src/App.tsx`
3. Create reusable components in `src/components/common/`
4. Use existing hooks and utilities before creating new ones

## Related Documentation

- [Web UI Architecture](../docs/web-ui-architecture.md) - Detailed architecture decisions
- [API Documentation](../docs/api-overview.md) - Backend API reference
- [Main README](../README.md) - Attune platform overview
