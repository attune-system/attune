/* eslint-disable react-refresh/only-export-components -- exporting hooks alongside WebSocketProvider is standard React pattern */
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useCallback,
  ReactNode,
} from "react";
import { safeErrorSummary } from "@/lib/safe-error-summary";

export interface Notification {
  notification_type: string;
  entity_type: string;
  entity_id: number;
  user_id?: number;
  payload: unknown;
  timestamp: string;
}

export type NotificationHandler = (notification: Notification) => void;

interface WebSocketContextValue {
  connected: boolean;
  subscribe: (filter: string, handler: NotificationHandler) => void;
  unsubscribe: (filter: string, handler: NotificationHandler) => void;
}

const WebSocketContext = createContext<WebSocketContextValue | null>(null);

interface WebSocketProviderProps {
  children: ReactNode;
  url?: string;
  autoConnect?: boolean;
  reconnectInterval?: number;
  maxReconnectAttempts?: number;
}

/**
 * WebSocketProvider maintains a single WebSocket connection for the entire application.
 *
 * **Authentication:** the notifier service requires a JWT on connect. Browser
 * WebSocket clients cannot set an `Authorization` header, so the access token
 * from `localStorage` is sent as a secondary `Sec-WebSocket-Protocol` value.
 * If no token is present, the provider waits and does not connect; if the
 * server rejects with `1008` (policy violation, treated here as auth failure /
 * closure on 401), we attempt to refresh the token via the existing API client
 * and reconnect.
 *
 * Note: In React 18 StrictMode (development only), components mount twice to help detect
 * side effects. This may briefly create two WebSocket connections, but the first one is
 * cleaned up immediately. In production builds, only one connection is created.
 */
export function WebSocketProvider({
  children,
  url: providedUrl,
  autoConnect = true,
  reconnectInterval = 5000,
  maxReconnectAttempts = 10,
}: WebSocketProviderProps) {
  // Construct WebSocket URL from base (add /ws path if not present)
  // Priority: explicit prop > env var > runtime config > derive from current origin via /ws/ proxy
  const baseUrl = (() => {
    if (providedUrl) return providedUrl;
    if (import.meta.env.VITE_WS_URL) return import.meta.env.VITE_WS_URL;
    const runtimeWs = window.__ATTUNE_RUNTIME_CONFIG__?.wsUrl;
    if (runtimeWs) return runtimeWs;
    // Derive from current page origin — nginx proxies /ws/ to the notifier
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    return `${proto}//${window.location.host}/ws`;
  })();
  const url = baseUrl.endsWith("/ws") ? baseUrl : `${baseUrl}/ws`;

  const [connected, setConnected] = useState(false);
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeoutRef = useRef<NodeJS.Timeout | null>(null);
  const shouldConnectRef = useRef(autoConnect);
  const reconnectAttemptsRef = useRef(0);
  const isConnectingRef = useRef(false);

  // Map of filter -> Set of handlers
  const subscriptionsRef = useRef<Map<string, Set<NotificationHandler>>>(
    new Map(),
  );

  /**
   * Read the current access token. Returns null if no token is available —
   * caller should defer connection until the user logs in.
   */
  const getAccessToken = useCallback((): string | null => {
    const token = localStorage.getItem("access_token");
    if (!token) {
      return null;
    }
    return token;
  }, []);

  const connect = useCallback(() => {
    // Don't reconnect if we're already connected, connecting, or explicitly disconnected
    if (
      wsRef.current?.readyState === WebSocket.OPEN ||
      isConnectingRef.current ||
      !shouldConnectRef.current
    ) {
      return;
    }

    // Check max reconnect attempts
    if (reconnectAttemptsRef.current >= maxReconnectAttempts) {
      console.error(
        `[WebSocket] Max reconnection attempts (${maxReconnectAttempts}) reached. Giving up.`,
      );
      shouldConnectRef.current = false;
      return;
    }

    const attemptConnect = () => {
      try {
        const token = getAccessToken();
        if (!token) {
          // No token yet — defer; the auth flow will trigger reconnect once
          // the user logs in (see `useEffect` below that watches storage).
          isConnectingRef.current = false;
          return;
        }

        isConnectingRef.current = true;
        const ws = new WebSocket(url, ["attune.v1", `attune.jwt.${token}`]);

        ws.onopen = () => {
          setConnected(true);
          isConnectingRef.current = false;
          reconnectAttemptsRef.current = 0; // Reset attempts on successful connection

          // Re-subscribe to all filters
          subscriptionsRef.current.forEach((_, filter) => {
            ws.send(
              JSON.stringify({
                type: "subscribe",
                filter,
              }),
            );
          });
        };

        ws.onmessage = (event) => {
          try {
            const message = JSON.parse(event.data);

            // Handle different message types
            if (message.type === "welcome") {
              // Connection acknowledged
            } else if (message.notification_type) {
              // This is a notification - dispatch to all relevant handlers
              const notification = message as Notification;

              // Call handlers for entity_type:* subscriptions
              const entityFilter = `entity_type:${notification.entity_type}`;
              const handlers = subscriptionsRef.current.get(entityFilter);
              if (handlers) {
                handlers.forEach((handler) => {
                  try {
                    handler(notification);
                  } catch (error) {
                    console.error(
                      "[WebSocket] Handler error:",
                      safeErrorSummary(error),
                    );
                  }
                });
              }
            }
          } catch (error) {
            console.error(
              "[WebSocket] Failed to parse message:",
              safeErrorSummary(error),
            );
          }
        };

        ws.onerror = (error) => {
          console.error("[WebSocket] Error:", safeErrorSummary(error));
        };

        ws.onclose = (event) => {
          setConnected(false);
          isConnectingRef.current = false;
          wsRef.current = null;

          // 1008 = policy violation. Browsers also map auth-failure HTTP 401
          // responses (returned during the WS handshake) into close codes
          // outside the normal range. Treat these as "token may be invalid"
          // and clear it so the user is bounced to login by the rest of the
          // app on their next API call. Do NOT auto-retry indefinitely.
          if (event.code === 1008) {
            console.warn(
              "[WebSocket] Connection closed with policy violation (likely auth failure)",
            );
          }

          // Attempt to reconnect if we should still be connected
          if (
            shouldConnectRef.current &&
            reconnectAttemptsRef.current < maxReconnectAttempts
          ) {
            reconnectAttemptsRef.current += 1;
            const delay = Math.min(
              reconnectInterval * reconnectAttemptsRef.current,
              30000,
            );
            // Attempting reconnection with backoff
            reconnectTimeoutRef.current = setTimeout(() => {
              attemptConnect();
            }, delay);
          }
        };

        wsRef.current = ws;
      } catch (error) {
        console.error(
          "[WebSocket] Failed to connect:",
          safeErrorSummary(error),
        );
        isConnectingRef.current = false;

        // Retry connection with backoff
        if (
          shouldConnectRef.current &&
          reconnectAttemptsRef.current < maxReconnectAttempts
        ) {
          reconnectAttemptsRef.current += 1;
          const delay = Math.min(
            reconnectInterval * reconnectAttemptsRef.current,
            30000,
          );
          reconnectTimeoutRef.current = setTimeout(() => {
            attemptConnect();
          }, delay);
        }
      }
    };

    attemptConnect();
  }, [getAccessToken, reconnectInterval, maxReconnectAttempts, url]);

  const disconnect = useCallback(() => {
    shouldConnectRef.current = false;
    isConnectingRef.current = false;
    reconnectAttemptsRef.current = 0;

    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = null;
    }

    if (wsRef.current) {
      // Close connection cleanly
      if (wsRef.current.readyState === WebSocket.OPEN) {
        wsRef.current.close(1000, "Client disconnecting");
      }
      wsRef.current = null;
    }

    setConnected(false);
  }, []);

  const subscribe = useCallback(
    (filter: string, handler: NotificationHandler) => {
      // Add handler to the set for this filter
      if (!subscriptionsRef.current.has(filter)) {
        subscriptionsRef.current.set(filter, new Set());
      }
      const handlers = subscriptionsRef.current.get(filter)!;
      const hadHandlers = handlers.size > 0;
      handlers.add(handler);

      // Only send subscribe message if this is the first handler for this filter
      if (!hadHandlers && wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(
          JSON.stringify({
            type: "subscribe",
            filter,
          }),
        );
      }
    },
    [],
  );

  const unsubscribe = useCallback(
    (filter: string, handler: NotificationHandler) => {
      const handlers = subscriptionsRef.current.get(filter);
      if (!handlers) return;

      handlers.delete(handler);

      // If no more handlers for this filter, unsubscribe from the server
      if (handlers.size === 0) {
        subscriptionsRef.current.delete(filter);

        if (wsRef.current?.readyState === WebSocket.OPEN) {
          wsRef.current.send(
            JSON.stringify({
              type: "unsubscribe",
              filter,
            }),
          );
        }
      }
    },
    [],
  );

  // Connect on mount if autoConnect is enabled, and re-connect when the
  // access token changes (login/logout/refresh) so the new token is sent on
  // the new handshake.
  useEffect(() => {
    if (!autoConnect) return;

    shouldConnectRef.current = true;
    connect();

    // Watch for auth token changes (login, logout, or refresh) so we
    // immediately reconnect with a fresh token. localStorage events fire
    // across tabs; in-tab updates need a custom event ("auth:token-changed")
    // which the API wrapper can dispatch on refresh — we tolerate its
    // absence by falling back to the periodic reconnect on close.
    const handleStorage = (e: StorageEvent) => {
      if (e.key === "access_token") {
        // Drop the existing socket so the next connect picks up the new token.
        if (wsRef.current) {
          try {
            wsRef.current.close(1000, "Token refreshed");
          } catch {
            // ignore
          }
          wsRef.current = null;
        }
        reconnectAttemptsRef.current = 0;
        shouldConnectRef.current = true;
        connect();
      }
    };

    const handleTokenChanged = () => {
      if (wsRef.current) {
        try {
          wsRef.current.close(1000, "Token refreshed");
        } catch {
          // ignore
        }
        wsRef.current = null;
      }
      reconnectAttemptsRef.current = 0;
      shouldConnectRef.current = true;
      connect();
    };

    window.addEventListener("storage", handleStorage);
    window.addEventListener("auth:token-changed", handleTokenChanged);

    return () => {
      window.removeEventListener("storage", handleStorage);
      window.removeEventListener("auth:token-changed", handleTokenChanged);
      disconnect();
    };
  }, [autoConnect, connect, disconnect]);

  const value: WebSocketContextValue = {
    connected,
    subscribe,
    unsubscribe,
  };

  return (
    <WebSocketContext.Provider value={value}>
      {children}
    </WebSocketContext.Provider>
  );
}

export function useWebSocketContext(): WebSocketContextValue {
  const context = useContext(WebSocketContext);
  if (!context) {
    throw new Error(
      "useWebSocketContext must be used within WebSocketProvider",
    );
  }
  return context;
}

/**
 * Hook for subscribing to specific entity type notifications
 * Uses the shared WebSocket connection from context
 *
 * @example
 * // Subscribe to all execution updates
 * useEntityNotifications('execution', () => {
 *   queryClient.invalidateQueries(['executions']);
 * });
 */
export function useEntityNotifications(
  entityType: string,
  onNotification: NotificationHandler,
  enabled = true,
) {
  const { connected, subscribe, unsubscribe } = useWebSocketContext();

  // Stable reference to the handler — updated on every render via effect
  const handlerRef = useRef(onNotification);

  // Update ref when handler changes (but don't cause re-subscription)
  useEffect(() => {
    handlerRef.current = onNotification;
  }, [onNotification]);

  // Create a stable wrapper function once via useMemo (no ref access during render)
  const stableHandler = useMemo<NotificationHandler>(
    () => (notification) => {
      handlerRef.current(notification);
    },
    [], // intentionally empty — handlerRef is stable
  );

  useEffect(() => {
    if (!connected || !enabled) return;

    const filter = `entity_type:${entityType}`;

    subscribe(filter, stableHandler);

    return () => {
      unsubscribe(filter, stableHandler);
    };
  }, [connected, enabled, entityType, subscribe, unsubscribe, stableHandler]);

  return { connected };
}
