# WebSocket Usage in Web UI

For the server protocol and exact payload for every stream, see the [Notifier WebSocket reference](../api/notifier-websocket.md).

## Overview

The Attune web UI uses a **single shared WebSocket connection** for real-time notifications across all pages. This connection is managed by `WebSocketProvider` and accessed via React hooks.

## Architecture

```
App.tsx
  └── WebSocketProvider (manages single WS connection)
        ├── EventsPage (subscribes to "event" notifications)
        ├── ExecutionsPage (subscribes to "execution" notifications)
        └── DashboardPage (subscribes to multiple entity types)
```

Only **one WebSocket connection** is created per browser tab, regardless of how many pages subscribe to notifications.

## Basic Usage

### 1. Ensure Provider is Configured

The `WebSocketProvider` should be set up in `App.tsx` (already configured):

```typescript
import { WebSocketProvider } from "@/contexts/WebSocketContext";

function App() {
  return (
    <WebSocketProvider>
      {/* Your app routes */}
    </WebSocketProvider>
  );
}
```

### 2. Subscribe to Entity Notifications

Use `useEntityNotifications` to receive real-time updates for a specific entity type:

```typescript
import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useEntityNotifications } from "@/contexts/WebSocketContext";

function MyPage() {
  const queryClient = useQueryClient();
  
  // IMPORTANT: Wrap handler in useCallback for stable reference
  const handleNotification = useCallback(() => {
    // Invalidate queries to refetch data
    queryClient.invalidateQueries({ queryKey: ["myEntity"] });
  }, [queryClient]);
  
  const { connected } = useEntityNotifications("myEntity", handleNotification);
  
  return (
    <div>
      {connected && <span>Live updates enabled</span>}
      {/* Rest of your component */}
    </div>
  );
}
```

## Available Entity Types

Subscribe to these entity types to receive notifications:

- `"event"` - Event creation/updates
- `"execution"` - Execution status changes
- `"enforcement"` - Rule enforcement events
- `"inquiry"` - Human-in-the-loop interactions
- `"artifact"` - Artifact creation, versions, and progress
- `"work_queue"` - Work queue creation and updates
- `"work_queue_item"` - Queued work creation and updates
- `"rule_lifecycle"` - Rule creation, enablement, disablement, and deletion

## Advanced Usage

### Multiple Subscriptions in One Component

```typescript
function DashboardPage() {
  const queryClient = useQueryClient();
  
  const handleEventNotification = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ["events"] });
  }, [queryClient]);
  
  const handleExecutionNotification = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ["executions"] });
  }, [queryClient]);
  
  useEntityNotifications("event", handleEventNotification);
  useEntityNotifications("execution", handleExecutionNotification);
  
  // Both subscriptions share the same WebSocket connection
}
```

### Custom Notification Handling

```typescript
import { useCallback } from "react";
import { useEntityNotifications, Notification } from "@/contexts/WebSocketContext";

function MyPage() {
  const handleNotification = useCallback((notification: Notification) => {
    console.log("Received notification:", notification);
    
    // Access notification details
    console.log("Entity type:", notification.entity_type);
    console.log("Entity ID:", notification.entity_id);
    console.log("Payload:", notification.payload);
    console.log("Timestamp:", notification.timestamp);
    
    // Custom logic based on notification
    if (notification.entity_id === specificId) {
      // Do something specific
    }
  }, []);
  
  useEntityNotifications("execution", handleNotification);
}
```

### Conditional Subscriptions

```typescript
function MyPage({ entityType }: { entityType: string }) {
  const [enabled, setEnabled] = useState(true);
  
  const handleNotification = useCallback(() => {
    // Handle notification
  }, []);
  
  // Only subscribe when enabled is true
  useEntityNotifications(entityType, handleNotification, enabled);
}
```

### Direct Context Access

For advanced use cases, access the WebSocket context directly:

```typescript
import { useWebSocketContext } from "@/contexts/WebSocketContext";

function AdvancedComponent() {
  const { connected, subscribe, unsubscribe } = useWebSocketContext();
  
  useEffect(() => {
    const handler = (notification) => {
      // Custom handling
    };
    
    // Subscribe to a custom filter
    subscribe("entity_type:execution", handler);
    
    return () => {
      unsubscribe("entity_type:execution", handler);
    };
  }, [subscribe, unsubscribe]);
}
```

## Important Guidelines

### ✅ DO

- **Always wrap handlers in `useCallback`** to prevent re-subscriptions
- Include all dependencies in the `useCallback` dependency array
- Use `queryClient.invalidateQueries` for simple cache invalidation
- Show connection status in the UI when relevant

### ❌ DON'T

- Don't create inline functions as handlers (causes re-subscriptions)
- Don't create multiple WebSocket connections (use the context)
- Don't forget to handle the `connected` state
- Don't use the deprecated `useWebSocket` from `@/hooks/useWebSocket.ts`

## Connection Status

Display connection status to users:

```typescript
function MyPage() {
  const { connected } = useEntityNotifications("event", handleNotification);
  
  return (
    <div>
      {connected ? (
        <div className="flex items-center gap-2 text-green-600">
          <div className="w-2 h-2 bg-green-600 rounded-full animate-pulse" />
          <span>Live updates</span>
        </div>
      ) : (
        <div className="text-gray-400">Connecting...</div>
      )}
    </div>
  );
}
```

## Configuration

WebSocket connection is configured via environment variables:

```bash
# .env.local or .env.development
VITE_WS_URL=ws://localhost:8081
```

The provider automatically appends `/ws` to the URL if not present.

## Troubleshooting

### Multiple Connections Opening

**Problem**: Browser DevTools shows multiple WebSocket connections.

**Solution**: 
- Ensure `WebSocketProvider` is only added once in `App.tsx`
- Check that handlers are wrapped in `useCallback`
- Verify React StrictMode isn't causing double-mounting in development

### Notifications Not Received

**Problem**: Component doesn't receive notifications.

**Checklist**:
1. Is `WebSocketProvider` wrapping your app?
2. Is the notifier service running? (`make run-notifier`)
3. Is `connected` returning `true`?
4. Is the entity type spelled correctly?
5. Are notifications actually being sent? (check server logs)

### Connection Keeps Reconnecting

**Problem**: WebSocket disconnects and reconnects repeatedly.

**Possible causes**:
- Notifier service restarting
- Network issues
- Authentication token missing or expired
- Check console for error messages

## Example: Complete Page

```typescript
import { useState, useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useEntityNotifications } from "@/contexts/WebSocketContext";
import { useEvents } from "@/hooks/useEvents";

export default function EventsPage() {
  const queryClient = useQueryClient();
  const [page, setPage] = useState(1);
  
  // Real-time updates
  const handleEventNotification = useCallback(() => {
    queryClient.invalidateQueries({ queryKey: ["events"] });
  }, [queryClient]);
  
  const { connected } = useEntityNotifications("event", handleEventNotification);
  
  // Fetch data
  const { data, isLoading } = useEvents({ page, pageSize: 20 });
  
  return (
    <div>
      <div className="flex items-center justify-between">
        <h1>Events</h1>
        {connected && (
          <div className="text-green-600">
            <div className="w-2 h-2 bg-green-600 rounded-full animate-pulse" />
            Live updates
          </div>
        )}
      </div>
      
      {/* Render events */}
    </div>
  );
}
```

## Performance Notes

- The single shared connection significantly reduces network overhead
- Subscriptions are reference-counted (only subscribe to each filter once)
- Handlers are called synchronously when notifications arrive
- No polling is needed - all updates are push-based via WebSocket

## Migration from Old Pattern

If you find code using the old pattern:

```typescript
// ❌ OLD - creates separate connection
import { useEntityNotifications } from "@/hooks/useWebSocket";
const { connected } = useEntityNotifications("event", () => {
  queryClient.invalidateQueries({ queryKey: ["events"] });
});
```

Update to:

```typescript
// ✅ NEW - uses shared connection
import { useEntityNotifications } from "@/contexts/WebSocketContext";
const handleNotification = useCallback(() => {
  queryClient.invalidateQueries({ queryKey: ["events"] });
}, [queryClient]);
const { connected } = useEntityNotifications("event", handleNotification);
```
