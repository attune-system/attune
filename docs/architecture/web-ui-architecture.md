# Web UI Architecture

**Created:** 2024-01-18  
**Status:** Planning  
**Tech Stack:** React 18 + TypeScript + Vite

## Overview

The Attune Web UI is a single-page application (SPA) that provides a comprehensive interface for managing and monitoring the Attune automation platform. It communicates with the Attune API service via REST endpoints and receives real-time updates through WebSocket connections to the notifier service.

## Architecture Principles

1. **Type Safety**: Full TypeScript coverage with types generated from OpenAPI specification
2. **Real-time Updates**: WebSocket integration for live execution monitoring and notifications
3. **Offline-first Caching**: Intelligent client-side caching with optimistic updates
4. **Component Reusability**: Shared components for common patterns (lists, forms, detail views)
5. **Performance**: Code splitting, lazy loading, and efficient re-rendering
6. **Developer Experience**: Fast dev server, hot module replacement, clear error messages

## Technology Stack

### Core Framework

#### **React 18**
- **Purpose**: UI component library and rendering engine
- **Why**: Industry standard, mature ecosystem, excellent for complex UIs
- **Key Features Used**:
  - Hooks for state management (useState, useEffect, useContext)
  - Suspense for code splitting and async data loading
  - Concurrent rendering for improved UX
  - Error boundaries for graceful error handling

#### **TypeScript 5.x**
- **Purpose**: Type safety and improved developer experience
- **Why**: Catches errors at compile time, better IDE support, self-documenting code
- **Configuration**:
  - Strict mode enabled
  - Path aliases for clean imports (@/components, @/api, @/hooks)
  - Integration with OpenAPI-generated types

#### **Vite**
- **Purpose**: Build tool and dev server
- **Why**: Fast HMR, optimized production builds, native ESM support
- **Features**:
  - Sub-second dev server startup
  - Instant hot module replacement
  - Optimized production builds with Rollup
  - Environment variable management
  - Plugin ecosystem (React, TypeScript support out of the box)

### API Layer

#### **OpenAPI TypeScript Codegen**
- **Purpose**: Generate type-safe API client from OpenAPI spec
- **Why**: Single source of truth, automatic updates when API changes
- **Generated Artifacts**:
  - TypeScript interfaces for all DTOs (requests/responses)
  - API client with methods for all 86 endpoints
   - Compile-time request types, not runtime request validation
  - Automatic bearer token injection

**Usage Pattern**:
```typescript
// Generated client usage
import { ActionsService } from '@/api/services/ActionsService';

// Type-safe API call with auto-complete
const actions = await ActionsService.listActions({
  limit: 50,
  offset: 0
});
// actions is typed as ApiResponse<PaginatedResponse<ActionSummary[]>>
```

**Code Generation Command**:
```bash
# Run whenever API spec changes
npm run generate:api

# Export the current Rust spec and check the client without writing files
npm run check:api
```

The generator is `web/scripts/generate-api.mjs`. See [API contract checks](../web-ui/api-contracts.md) for generation details and request tests.

**Configuration**:
- Client: axios (configurable for auth, interceptors)
- Type generation: all request/response types
- Service generation: one service class per tag (PacksService, ActionsService, etc.)
- Enum handling: TypeScript enums, with literal discriminated unions for inline patch operations

#### **Axios**
- **Purpose**: HTTP client for API requests
- **Why**: Interceptors for auth, request/response transformation, browser/node compatibility
- **Configuration**:
  - Base URL from environment variable
  - Request interceptor: inject JWT token from storage
  - Response interceptor: handle 401 (refresh token), network errors
  - Timeout configuration
  - Request/response logging in development

**Axios Instance Setup**:
```typescript
// src/api/client.ts
import axios from 'axios';
import { getAccessToken, refreshAccessToken } from '@/auth/tokens';

const apiClient = axios.create({
  baseURL: import.meta.env.VITE_API_URL || 'http://localhost:8080',
  timeout: 30000,
  headers: {
    'Content-Type': 'application/json',
  },
});

// Request interceptor: inject auth token
apiClient.interceptors.request.use((config) => {
  const token = getAccessToken();
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

// Response interceptor: handle auth errors
apiClient.interceptors.response.use(
  (response) => response,
  async (error) => {
    const originalRequest = error.config;
    
    // If 401 and not already retried, attempt token refresh
    if (error.response?.status === 401 && !originalRequest._retry) {
      originalRequest._retry = true;
      try {
        await refreshAccessToken();
        return apiClient(originalRequest);
      } catch (refreshError) {
        // Refresh failed, redirect to login
        window.location.href = '/login';
        return Promise.reject(refreshError);
      }
    }
    
    return Promise.reject(error);
  }
);
```

### Data Fetching & Caching

#### **TanStack Query (React Query v5)**
- **Purpose**: Server state management, caching, and synchronization
- **Why**: Eliminates boilerplate, automatic caching, background refetching, optimistic updates
- **Key Features**:
  - Automatic background refetching
  - Cache invalidation and updates
  - Optimistic updates for better UX
  - Request deduplication
  - Pagination and infinite scroll support
  - Prefetching for improved perceived performance

**Query Usage Patterns**:

```typescript
// src/hooks/useActions.ts
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { ActionsService } from '@/api/services/ActionsService';

// Query for list of actions
export function useActions(params?: { pack_ref?: string; limit?: number }) {
  return useQuery({
    queryKey: ['actions', params],
    queryFn: () => ActionsService.listActions(params),
    staleTime: 30000, // Consider fresh for 30s
    gcTime: 5 * 60 * 1000, // Keep in cache for 5 min
  });
}

// Query for single action
export function useAction(ref: string) {
  return useQuery({
    queryKey: ['actions', ref],
    queryFn: () => ActionsService.getActionByRef({ ref }),
    enabled: !!ref, // Only fetch if ref is provided
  });
}

// Mutation for creating action
export function useCreateAction() {
  const queryClient = useQueryClient();
  
  return useMutation({
    mutationFn: ActionsService.createAction,
    onSuccess: (newAction) => {
      // Invalidate action list to trigger refetch
      queryClient.invalidateQueries({ queryKey: ['actions'] });
      
      // Optimistically update cache with new action
      queryClient.setQueryData(['actions', newAction.ref], newAction);
    },
  });
}

// Mutation with optimistic update
export function useUpdateAction() {
  const queryClient = useQueryClient();
  
  return useMutation({
    mutationFn: ({ ref, data }: { ref: string; data: UpdateActionRequest }) =>
      ActionsService.updateAction({ ref, requestBody: data }),
    onMutate: async ({ ref, data }) => {
      // Cancel outgoing refetches
      await queryClient.cancelQueries({ queryKey: ['actions', ref] });
      
      // Snapshot previous value
      const previous = queryClient.getQueryData(['actions', ref]);
      
      // Optimistically update
      queryClient.setQueryData(['actions', ref], (old: any) => ({
        ...old,
        ...data,
      }));
      
      return { previous };
    },
    onError: (err, variables, context) => {
      // Rollback on error
      queryClient.setQueryData(['actions', variables.ref], context?.previous);
    },
    onSettled: (data, error, variables) => {
      // Refetch after mutation
      queryClient.invalidateQueries({ queryKey: ['actions', variables.ref] });
    },
  });
}
```

**Query Key Strategy**:
- Use hierarchical keys: `['resource', params]`
- Examples:
  - `['actions']` - all actions
  - `['actions', { pack_ref: 'core' }]` - actions filtered by pack
  - `['actions', 'core.http']` - specific action
  - `['executions', { status: 'running' }]` - filtered executions

**Cache Invalidation Patterns**:
- After mutations: invalidate related queries
- WebSocket updates: update specific cache entries
- Manual refresh: invalidate and refetch
- Periodic background updates for critical data (running executions)

### Authentication

#### **JWT Token Management**
- **Storage**: Access token in memory, refresh token in httpOnly cookie (if backend supports) or localStorage
- **Flow**:
  1. User logs in → receives access token (1h) + refresh token (7d)
  2. Store tokens securely
  3. Axios interceptor adds access token to all requests
  4. On 401 response, attempt refresh
  5. If refresh succeeds, retry original request
  6. If refresh fails, redirect to login

**Auth Context**:
```typescript
// src/contexts/AuthContext.tsx
import React, { createContext, useContext, useState, useEffect } from 'react';
import { AuthService } from '@/api/services/AuthService';

interface User {
  id: number;
  username: string;
  email: string;
  roles: string[];
}

interface AuthContextType {
  user: User | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  login: (username: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
  refreshUser: () => Promise<void>;
}

const AuthContext = createContext<AuthContextType | undefined>(undefined);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [user, setUser] = useState<User | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  
  // Load user on mount
  useEffect(() => {
    loadUser();
  }, []);
  
  const loadUser = async () => {
    try {
      const response = await AuthService.getCurrentUser();
      setUser(response.data);
    } catch (error) {
      setUser(null);
    } finally {
      setIsLoading(false);
    }
  };
  
  const login = async (username: string, password: string) => {
    const response = await AuthService.login({ requestBody: { username, password } });
    localStorage.setItem('access_token', response.data.access_token);
    localStorage.setItem('refresh_token', response.data.refresh_token);
    await loadUser();
  };
  
  const logout = async () => {
    localStorage.removeItem('access_token');
    localStorage.removeItem('refresh_token');
    setUser(null);
  };
  
  return (
    <AuthContext.Provider value={{
      user,
      isAuthenticated: !!user,
      isLoading,
      login,
      logout,
      refreshUser: loadUser,
    }}>
      {children}
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (!context) throw new Error('useAuth must be used within AuthProvider');
  return context;
}
```

**Protected Routes**:
```typescript
// src/components/ProtectedRoute.tsx
import { Navigate } from 'react-router-dom';
import { useAuth } from '@/contexts/AuthContext';

export function ProtectedRoute({ children }: { children: React.ReactNode }) {
  const { isAuthenticated, isLoading } = useAuth();
  
  if (isLoading) {
    return <LoadingSpinner />;
  }
  
  if (!isAuthenticated) {
    return <Navigate to="/login" replace />;
  }
  
  return <>{children}</>;
}
```

### Real-time Updates

#### **WebSocket Client**
- **Purpose**: Receive real-time notifications from notifier service
- **Connection**: WebSocket to notifier service (separate from API)
- **Protocol**: JSON messages with event types
- **Reconnection**: Automatic reconnection with exponential backoff

**WebSocket Integration**:
```typescript
// src/websocket/client.ts
import { useEffect, useRef } from 'react';
import { useQueryClient } from '@tanstack/react-query';

interface NotificationMessage {
  type: 'execution.started' | 'execution.completed' | 'execution.failed' | 
        'inquiry.created' | 'event.created';
  data: any;
  timestamp: string;
}

export function useWebSocketNotifications() {
  const wsRef = useRef<WebSocket | null>(null);
  const queryClient = useQueryClient();
  const reconnectTimeoutRef = useRef<NodeJS.Timeout>();
  const reconnectAttempts = useRef(0);
  
  useEffect(() => {
    const connect = () => {
      const wsUrl = import.meta.env.VITE_WS_URL || 'ws://localhost:8081';
      const token = localStorage.getItem('access_token');
      
      wsRef.current = new WebSocket(wsUrl, ['attune.v1', `attune.jwt.${token}`]);
      
      wsRef.current.onopen = () => {
        console.log('WebSocket connected');
        reconnectAttempts.current = 0;
      };
      
      wsRef.current.onmessage = (event) => {
        const message: NotificationMessage = JSON.parse(event.data);
        handleNotification(message);
      };
      
      wsRef.current.onclose = () => {
        console.log('WebSocket disconnected');
        scheduleReconnect();
      };
      
      wsRef.current.onerror = (error) => {
        console.error('WebSocket error:', error);
      };
    };
    
    const scheduleReconnect = () => {
      const delay = Math.min(1000 * Math.pow(2, reconnectAttempts.current), 30000);
      reconnectAttempts.current++;
      
      reconnectTimeoutRef.current = setTimeout(() => {
        connect();
      }, delay);
    };
    
    const handleNotification = (message: NotificationMessage) => {
      switch (message.type) {
        case 'execution.started':
        case 'execution.completed':
        case 'execution.failed':
          // Update execution in cache
          queryClient.setQueryData(
            ['executions', message.data.id],
            message.data
          );
          // Invalidate execution lists
          queryClient.invalidateQueries({ queryKey: ['executions'] });
          break;
          
        case 'inquiry.created':
          // Add notification to UI
          queryClient.invalidateQueries({ queryKey: ['inquiries'] });
          break;
          
        case 'event.created':
          // Update event stream
          queryClient.invalidateQueries({ queryKey: ['events'] });
          break;
      }
    };
    
    connect();
    
    return () => {
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, [queryClient]);
}
```

**Usage in App**:
```typescript
// src/App.tsx
function App() {
  useWebSocketNotifications(); // Connect once at app level
  
  return (
    <AuthProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </AuthProvider>
  );
}
```

### Workflow Visualization

#### **React Flow**
- **Purpose**: Visual workflow editor and execution graph display
- **Why**: Best-in-class workflow visualization library for React
- **Features**:
  - Drag-and-drop node creation
  - Custom node types (action, decision, parallel, inquiry)
  - Edge routing and validation
  - Minimap and controls
  - Export to image
  - Zoom and pan

**Workflow Editor Example**:
```typescript
// src/components/WorkflowEditor.tsx
import ReactFlow, { 
  Background, 
  Controls, 
  MiniMap,
  addEdge,
  useNodesState,
  useEdgesState,
} from 'reactflow';
import 'reactflow/dist/style.css';

const nodeTypes = {
  action: ActionNode,
  decision: DecisionNode,
  parallel: ParallelNode,
  inquiry: InquiryNode,
};

export function WorkflowEditor({ workflow }: { workflow: Workflow }) {
  const [nodes, setNodes, onNodesChange] = useNodesState(
    convertWorkflowToNodes(workflow)
  );
  const [edges, setEdges, onEdgesChange] = useEdgesState(
    convertWorkflowToEdges(workflow)
  );
  
  const onConnect = useCallback(
    (params) => setEdges((eds) => addEdge(params, eds)),
    [setEdges]
  );
  
  return (
    <div style={{ height: '600px' }}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        nodeTypes={nodeTypes}
        fitView
      >
        <Background />
        <Controls />
        <MiniMap />
      </ReactFlow>
    </div>
  );
}
```

**Custom Node Types**:
- **ActionNode**: Represents an action execution with status indicator
- **DecisionNode**: Conditional branching with expression display
- **ParallelNode**: Concurrent execution branches
- **InquiryNode**: Human-in-the-loop interaction points

### Code/YAML Editing

#### **Monaco Editor**
- **Purpose**: Rich code/YAML editor for workflows, rules, and configurations
- **Why**: Same editor as VS Code, excellent language support
- **Features**:
  - Syntax highlighting for YAML, JSON, Python, JavaScript
  - Auto-completion
  - Error detection and linting
  - Diff editor for comparing versions
  - Themes (light/dark)

**Monaco Integration**:
```typescript
// src/components/CodeEditor.tsx
import Editor from '@monaco-editor/react';

interface CodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  language: 'yaml' | 'json' | 'python' | 'javascript';
  readOnly?: boolean;
}

export function CodeEditor({ 
  value, 
  onChange, 
  language, 
  readOnly = false 
}: CodeEditorProps) {
  return (
    <Editor
      height="400px"
      language={language}
      value={value}
      onChange={(val) => onChange(val || '')}
      theme="vs-dark"
      options={{
        readOnly,
        minimap: { enabled: false },
        lineNumbers: 'on',
        scrollBeyondLastLine: false,
        automaticLayout: true,
      }}
    />
  );
}
```

**YAML Validation**:
```typescript
// src/utils/yamlValidation.ts
import yaml from 'js-yaml';
import Ajv from 'ajv';

export function validateWorkflowYAML(content: string): ValidationResult {
  try {
    const parsed = yaml.load(content);
    // Validate against workflow schema
    const ajv = new Ajv();
    const valid = ajv.validate(workflowSchema, parsed);
    
    if (!valid) {
      return { valid: false, errors: ajv.errors };
    }
    
    return { valid: true, data: parsed };
  } catch (error) {
    return { 
      valid: false, 
      errors: [{ message: error.message }] 
    };
  }
}
```

### UI Components

#### **shadcn/ui**
- **Purpose**: High-quality, accessible component library
- **Why**: Copy-paste components (no NPM bloat), fully customizable, Tailwind-based
- **Components Used**:
  - Button, Input, Select, Checkbox
  - Table, Card, Dialog, Popover
  - Tabs, Accordion, Sheet (side panel)
  - Toast (notifications)
  - Command (command palette)
  - Form (with react-hook-form integration)

**Installation**: Components are copied into project, not installed as dependency

**Usage Pattern**:
```typescript
// src/components/actions/ActionList.tsx
import { Button } from '@/components/ui/button';
import { Table } from '@/components/ui/table';
import { useActions } from '@/hooks/useActions';

export function ActionList() {
  const { data, isLoading } = useActions();
  
  if (isLoading) return <Skeleton />;
  
  return (
    <div>
      <div className="flex justify-between mb-4">
        <h1>Actions</h1>
        <Button onClick={handleCreate}>Create Action</Button>
      </div>
      
      <Table>
        {/* Table content */}
      </Table>
    </div>
  );
}
```

#### **Tailwind CSS**
- **Purpose**: Utility-first CSS framework
- **Why**: Fast styling, consistent design system, small production bundle
- **Configuration**:
  - Custom color palette matching Attune brand
  - Dark mode support
  - Responsive breakpoints
  - Custom animations for loading states

### Routing

#### **React Router v6**
- **Purpose**: Client-side routing and navigation
- **Why**: Standard React routing solution, type-safe with TypeScript
- **Features**:
  - Nested routes
  - Lazy loading for code splitting
  - Protected routes
  - URL parameter handling

**Route Structure**:
```typescript
// src/router.tsx
import { createBrowserRouter } from 'react-router-dom';

export const router = createBrowserRouter([
  {
    path: '/login',
    element: <LoginPage />,
  },
  {
    path: '/',
    element: <ProtectedRoute><Layout /></ProtectedRoute>,
    children: [
      { index: true, element: <DashboardPage /> },
      
      // Packs
      { path: 'packs', element: <PackListPage /> },
      { path: 'packs/:ref', element: <PackDetailPage /> },
      
      // Actions
      { path: 'actions', element: <ActionListPage /> },
      { path: 'actions/:ref', element: <ActionDetailPage /> },
      { path: 'actions/:ref/edit', element: <ActionEditPage /> },
      
      // Rules
      { path: 'rules', element: <RuleListPage /> },
      { path: 'rules/:ref', element: <RuleDetailPage /> },
      
      // Workflows
      { path: 'workflows', element: <WorkflowListPage /> },
      { path: 'workflows/:ref', element: <WorkflowDetailPage /> },
      { path: 'workflows/:ref/edit', element: <WorkflowEditorPage /> },
      
      // Executions
      { path: 'executions', element: <ExecutionListPage /> },
      { path: 'executions/:id', element: <ExecutionDetailPage /> },
      
      // Events & Enforcements
      { path: 'events', element: <EventListPage /> },
      { path: 'enforcements', element: <EnforcementListPage /> },
      
      // Inquiries
      { path: 'inquiries', element: <InquiryListPage /> },
      { path: 'inquiries/:id', element: <InquiryDetailPage /> },
      
      // Settings
      { path: 'settings', element: <SettingsPage /> },
      { path: 'settings/secrets', element: <SecretsPage /> },
    ],
  },
]);
```

### State Management

**Approach**: Minimal global state, prefer server state (React Query) and component state

**Global State (if needed)**:
- **Zustand**: Lightweight state management for truly global UI state
- Use cases:
  - Theme preference (light/dark)
  - Sidebar collapsed/expanded state
  - User preferences
  - Command palette open/closed

```typescript
// src/stores/uiStore.ts
import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface UIState {
  theme: 'light' | 'dark';
  sidebarCollapsed: boolean;
  toggleTheme: () => void;
  toggleSidebar: () => void;
}

export const useUIStore = create<UIState>()(
  persist(
    (set) => ({
      theme: 'dark',
      sidebarCollapsed: false,
      toggleTheme: () => set((state) => ({ 
        theme: state.theme === 'light' ? 'dark' : 'light' 
      })),
      toggleSidebar: () => set((state) => ({ 
        sidebarCollapsed: !state.sidebarCollapsed 
      })),
    }),
    { name: 'attune-ui-store' }
  )
);
```

## Project Structure

```
attune-web/
├── public/
│   ├── favicon.ico
│   └── logo.svg
├── src/
│   ├── api/                      # OpenAPI generated code
│   │   ├── models/              # TypeScript interfaces for all DTOs
│   │   ├── services/            # API service classes
│   │   └── core/                # Axios client configuration
│   ├── components/
│   │   ├── ui/                  # shadcn/ui base components
│   │   ├── layout/              # Layout components (Sidebar, Header, etc.)
│   │   ├── actions/             # Action-related components
│   │   ├── rules/               # Rule-related components
│   │   ├── workflows/           # Workflow editor and viewer
│   │   ├── executions/          # Execution monitoring
│   │   ├── events/              # Event stream
│   │   └── common/              # Shared components
│   ├── hooks/                   # Custom React hooks
│   │   ├── useActions.ts
│   │   ├── useRules.ts
│   │   ├── useWorkflows.ts
│   │   ├── useExecutions.ts
│   │   └── useWebSocket.ts
│   ├── contexts/                # React contexts
│   │   └── AuthContext.tsx
│   ├── stores/                  # Zustand stores (minimal)
│   │   └── uiStore.ts
│   ├── pages/                   # Page components
│   │   ├── DashboardPage.tsx
│   │   ├── LoginPage.tsx
│   │   ├── actions/
│   │   ├── rules/
│   │   ├── workflows/
│   │   └── executions/
│   ├── websocket/               # WebSocket client
│   │   └── client.ts
│   ├── utils/                   # Utility functions
│   │   ├── formatters.ts
│   │   ├── validators.ts
│   │   └── yaml.ts
│   ├── types/                   # Additional TypeScript types
│   ├── router.tsx               # React Router configuration
│   ├── App.tsx                  # Root component
│   └── main.tsx                 # Entry point
├── .env.development             # Dev environment variables
├── .env.production              # Prod environment variables
├── index.html
├── package.json
├── tsconfig.json
├── vite.config.ts
└── tailwind.config.js
```

## Development Workflow

### Initial Setup

```bash
# Create React + TypeScript project with Vite
npm create vite@latest attune-web -- --template react-ts
cd attune-web

# Install core dependencies
npm install react-router-dom @tanstack/react-query axios
npm install @monaco-editor/react reactflow zustand js-yaml

# Install UI dependencies
npm install -D tailwindcss postcss autoprefixer
npx tailwindcss init -p

# Install dev dependencies
npm install -D @types/node openapi-typescript-codegen

# Initialize shadcn/ui
npx shadcn-ui@latest init
```

### Generate API Client

```bash
# Generate TypeScript client from OpenAPI spec
# Run this whenever the API changes
npm run generate:api
```

**package.json script**:
```json
{
  "scripts": {
    "generate:api": "openapi-typescript-codegen --input http://localhost:8080/api-spec/openapi.json --output ./src/api --client axios --useOptions"
  }
}
```

### Development Server

```bash
# Start dev server with HMR
npm run dev

# Build for production
npm run build

# Preview production build
npm run preview
```

### Environment Variables

**`.env.development`**:
```env
VITE_API_URL=http://localhost:8080
VITE_WS_URL=ws://localhost:8081
VITE_LOG_LEVEL=debug
```

**`.env.production`**:
```env
VITE_API_URL=https://api.attune.example.com
VITE_WS_URL=wss://notifications.attune.example.com
VITE_LOG_LEVEL=error
```

## Form Management and Entity Editability

### Pack-Based vs UI-Configurable Components

Attune uses a **pack-based architecture** where most automation components are defined as code and bundled into packs. The Web UI must respect these architectural constraints when providing forms.

#### Code-Based Components (NOT UI-Editable)

**Actions**:
- Implemented as executable code (Python, Node.js, Shell)
- Registered when a pack is loaded/installed
- **No create/edit forms** in Web UI
- Managed through pack lifecycle (install, update, uninstall)
- Rationale: Security, performance, code quality, testing requirements

**Sensors**:
- Implemented as executable code with event monitoring logic
- Registered when a pack is loaded/installed
- **No create/edit forms** in Web UI
- Managed through pack lifecycle
- Rationale: Require event loop integration, complex dependencies, safety

#### Mixed Model Components

**Triggers**:
- **Pack-based triggers**: Registered with system packs (e.g., `slack.message_received`)
  - **NOT UI-editable** - defined in pack manifest
- **Ad-hoc triggers**: For custom integrations without code
  - **UI-editable** via trigger form (future feature)
  - Only for ad-hoc packs (`system: false`)
  - Define parameters schema and payload schema

#### Always UI-Configurable Components

**Rules**:
- Connect triggers to actions with criteria and parameters
- No code execution, just data mapping
- **Full CRUD operations** via Web UI
- Users need flexibility to change business logic

**Packs (Ad-Hoc)**:
- User-created packs for custom automation
- **Full CRUD operations** via Web UI
- Define configuration schema (JSON Schema format)
- No code required

**Workflows** (Future):
- Multi-step automation sequences
- Visual workflow editor (React Flow)
- Workflow actions (special configurable actions)

### Form Implementation Guidelines

**Required Forms**:
1. ✅ **RuleForm** (`/rules/new`, `/rules/:id/edit`) - Implemented
2. ✅ **PackForm** (`/packs/new`, `/packs/:name/edit`) - Implemented
3. 🔄 **TriggerForm** (`/triggers/new`, `/triggers/:id/edit`) - Future
   - Only for ad-hoc packs
   - Validate pack is non-system before allowing trigger creation
4. 🔄 **WorkflowForm** (`/workflows/new`, `/workflows/:ref/edit`) - Future

**NOT Required**:
- ❌ ActionForm - Actions are code-based
- ❌ SensorForm - Sensors are code-based

### Form Validation Strategy

**Client-Side Validation**:
- Required field checks
- Format validation (names, versions, JSON syntax)
- JSON Schema validation for configuration schemas
- Real-time error display with field-level messages

**Server-Side Validation**:
- API error capture and display
- Generic error handling for network failures
- Field-specific errors when available

### Form State Management

```typescript
// Example: RuleForm component structure
function RuleForm({ rule, onSuccess, onCancel }: RuleFormProps) {
  const isEditing = !!rule;
  
  // Local form state
  const [packId, setPackId] = useState(rule?.pack_id || 0);
  const [triggerId, setTriggerId] = useState(rule?.trigger_id || 0);
  const [actionId, setActionId] = useState(rule?.action_id || 0);
  const [errors, setErrors] = useState<Record<string, string>>({});
  
  // Data fetching
  const { data: packs } = usePacks();
  const { data: triggers } = usePackTriggers(selectedPackName);
  const { data: actions } = usePackActions(selectedPackName);
  
  // Mutations
  const createRule = useCreateRule();
  const updateRule = useUpdateRule();
  
  // Validation and submission
  const validateForm = () => { /* ... */ };
  const handleSubmit = async (e) => { /* ... */ };
  
  return <form onSubmit={handleSubmit}>...</form>;
}
```

**Key Patterns**:
- Cascading dropdowns (pack → triggers/actions)
- Immutable fields when editing (pack, trigger, action IDs)
- JSON editors with syntax validation
- Optimistic UI updates after mutations
- Auto-navigation after successful creation

### Entity List Pages with Create Buttons

**Pattern**: List pages should have a prominent "Create" button when entity creation is allowed.

**Implemented**:
- ✅ Rules list page: "Create Rule" button → `/rules/new`
- ✅ Packs list page: "Register Pack" button → `/packs/new`

**Should NOT Have Create Buttons**:
- ❌ Actions list page - Actions are code-based
- ❌ Sensors list page - Sensors are code-based

**Future**:
- 🔄 Triggers list page: "Create Trigger" button (only shows for ad-hoc packs)
- 🔄 Workflows list page: "Create Workflow" button

See `docs/pack-management-architecture.md` for detailed architectural rationale.

---

## Key Features Implementation

### Dashboard

- **Real-time metrics**: Active executions, success/failure rates, event throughput
- **Recent activity**: Latest executions, events, and inquiries
- **Quick actions**: Common tasks (run workflow, create rule)
- **System health**: Service status indicators

### Pack Management

- **List view**: All packs with search and filters
- **Detail view**: Pack info, contained actions/rules/workflows
- **Create/Edit**: Form for pack metadata
- **Sync workflows**: Trigger workflow sync from pack directory

### Action Management

- **List view**: Searchable, filterable table of actions
- **Detail view**: Action parameters, metadata, associated rules
- **Create/Edit**: Form with parameter schema editor
- **Test runner**: Execute action with test parameters

### Rule Management

- **List view**: All rules with enable/disable toggle
- **Detail view**: Trigger, action, parameter mapping, criteria
- **Create/Edit**: Visual rule builder with expression editor
- **Testing**: Test rule against sample event payload

### Workflow Management

- **List view**: Workflows with status, tags, last execution
- **Visual editor**: React Flow-based workflow designer
- **YAML editor**: Monaco editor with validation
- **Dual mode**: Switch between visual and code editing
- **Execution history**: View past executions with drill-down

### Execution Monitoring

- **Live dashboard**: Real-time execution updates via WebSocket
- **Filterable list**: By status, action, pack, time range
- **Detail view**: Full execution context, logs, parent/child relationships
- **Retry/Cancel**: Actions on executions
- **Log streaming**: Real-time log output for running executions

### Event Stream

- **Live feed**: Real-time event stream
- **Filters**: By trigger, status, time range
- **Detail view**: Event payload, resulting enforcements
- **Replay**: Re-evaluate rules against past events

### Inquiry Management

- **Notification center**: Pending inquiries requiring action
- **Response interface**: Form for inquiry responses
- **History**: Past inquiries with responses

## Performance Optimizations

1. **Code Splitting**: Lazy load routes with `React.lazy()`
2. **Bundle Optimization**: Tree shaking, minification via Vite
3. **Image Optimization**: WebP format, lazy loading
4. **Query Caching**: Intelligent cache times based on data volatility
5. **Virtual Scrolling**: For large lists (executions, events)
6. **Debounced Search**: Avoid excessive API calls
7. **Optimistic Updates**: Immediate UI feedback on mutations
8. **Prefetching**: Prefetch likely next pages on hover

## Testing Strategy

### Unit Tests
- Component logic with React Testing Library
- Hook behavior with `@testing-library/react-hooks`
- Utility functions with Jest

### Integration Tests
- User flows (login, create workflow, monitor execution)
- API client mocking with MSW (Mock Service Worker)
- Form validation and submission

### E2E Tests
- Critical paths with Playwright
- Cross-browser testing
- Accessibility testing with axe-core

## Deployment

### Build Output
```bash
npm run build
# Outputs to dist/ directory
```

### Static Hosting
- Deploy `dist/` to any static host (Nginx, Cloudflare Pages, Vercel, Netlify)
- Configure routing: redirect all requests to `index.html` for SPA routing

### Docker Container
```dockerfile
FROM node:20-alpine AS builder
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
CMD ["nginx", "-g", "daemon off;"]
```

**nginx.conf**:
```nginx
server {
  listen 80;
  root /usr/share/nginx/html;
  index index.html;

  # SPA routing: redirect all to index.html
  location / {
    try_files $uri $uri/ /index.html;
  }

  # Cache static assets
  location ~* \.(js|css|png|jpg|jpeg|gif|svg|ico|woff|woff2|ttf|eot)$ {
    expires 1y;
    add_header Cache-Control "public, immutable";
  }
}
```

## Security Considerations

1. **XSS Prevention**: React's automatic escaping, CSP headers
2. **Token Storage**: Access token in memory (preferred) or localStorage with caution
3. **API Communication**: HTTPS only in production
4. **Input Validation**: Client-side validation + server-side enforcement
5. **CORS**: Proper CORS configuration on API service
6. **CSP Headers**: Content Security Policy to prevent injection attacks

## Accessibility

1. **Semantic HTML**: Proper heading hierarchy, landmarks
2. **ARIA Labels**: For interactive elements and dynamic content
3. **Keyboard Navigation**: All features accessible via keyboard
4. **Focus Management**: Clear focus indicators, focus trapping in modals
5. **Screen Reader Support**: Announcements for dynamic updates
6. **Color Contrast**: WCAG AA compliance minimum

## Browser Support

- **Modern Browsers**: Chrome, Firefox, Safari, Edge (latest 2 versions)
- **No IE Support**: Modern JavaScript and CSS features used

## Future Enhancements

1. **Progressive Web App**: Offline support, install prompt
2. **Internationalization**: i18n support with react-i18next
3. **Advanced Workflow Editor**: Template library, drag-from-palette
4. **Collaboration**: Multi-user editing indicators
5. **Telemetry**: Usage analytics and error tracking
6. **Mobile Responsive**: Optimized layouts for tablets and phones
7. **Command Palette**: Keyboard-driven navigation (Cmd+K)
8. **Export/Import**: Workflow and pack export in various formats

## Documentation for Developers

All developers working on the web UI should familiarize themselves with:
1. This architecture document
2. OpenAPI specification at `/api-spec/openapi.json`
3. React Query documentation for data fetching patterns
4. shadcn/ui component documentation
5. React Flow documentation for workflow editor

## Troubleshooting Common Issues

### API Client Out of Sync
**Problem**: TypeScript errors after API changes  
**Solution**: Regenerate client with `npm run generate:api`

### WebSocket Not Connecting
**Problem**: Real-time updates not working  
**Solution**: Check VITE_WS_URL, verify notifier service is running

### Authentication Loops
**Problem**: Constant redirects to login  
**Solution**: Check token expiry, verify refresh token mechanism

### Slow Initial Load
**Problem**: Large bundle size  
**Solution**: Review code splitting, check for unnecessary dependencies in bundle

---

This architecture provides a solid foundation for building a professional, performant, and maintainable web interface for the Attune automation platform.
