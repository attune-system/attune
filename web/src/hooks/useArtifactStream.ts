import { useCallback, useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  useEntityNotifications,
  type Notification,
} from "@/contexts/WebSocketContext";
import { scheduleArtifactRefresh } from "@/lib/artifact-refresh";

interface UseArtifactStreamOptions {
  /**
   * Optional artifact ID to filter updates for a specific artifact.
   * If not provided, receives updates for all artifacts.
   */
  artifactId?: number;

  /**
   * Optional execution ID to filter artifact updates for a specific execution.
   * If not provided, receives updates for all artifacts.
   */
  executionId?: number;

  /**
   * Whether the stream should be active.
   * Defaults to true.
   */
  enabled?: boolean;

  /**
   * Whether live artifact updates should be paused for this view.
   */
  paused?: boolean;
}

/** Shape of data coming from WebSocket notifications for artifacts */
interface ArtifactNotification {
  entity_id: number;
  entity_type: string;
  notification_type: string;
  payload: ArtifactNotificationPayload;
  timestamp: string;
}

/** The raw payload from the PostgreSQL trigger for artifact notifications */
interface ArtifactNotificationPayload {
  execution?: number;
  [key: string]: unknown;
}

/**
 * Hook to subscribe to real-time artifact updates via WebSocket.
 *
 * Listens to `artifact_created` and `artifact_updated` notifications from the
 * PostgreSQL LISTEN/NOTIFY system, and invalidates relevant React Query caches
 * so that artifact lists and detail views update in real time.
 *
 * Notifications serve as invalidations. Artifact data comes from the API.
 *
 * @example
 * ```tsx
 * // Listen to all artifact updates
 * useArtifactStream();
 *
 * // Listen to artifacts for a specific execution
 * useArtifactStream({ executionId: 123 });
 * ```
 */
export function useArtifactStream(options: UseArtifactStreamOptions = {}) {
  const { artifactId, executionId, enabled = true, paused = false } = options;
  const queryClient = useQueryClient();
  const active = useRef(enabled && !paused);
  useEffect(() => {
    active.current = enabled && !paused;
    return () => {
      active.current = false;
    };
  }, [enabled, paused]);
  const shouldRefresh = useCallback(() => active.current, []);

  const handleNotification = useCallback(
    (raw: Notification) => {
      if (paused) {
        return;
      }

      const notification = raw as unknown as ArtifactNotification;
      const payload = notification.payload;

      // If we're filtering by artifact ID, only process matching artifacts
      if (artifactId && notification.entity_id !== artifactId) {
        return;
      }

      // If we're filtering by execution ID, only process matching artifacts
      if (executionId && payload?.execution !== executionId) {
        return;
      }

      const updatedArtifactId = notification.entity_id;
      const artifactExecution = payload?.execution;

      // Refresh artifact list caches so list pages pick up created/updated artifacts.
      scheduleArtifactRefresh(
        queryClient,
        ["artifacts", "list"],
        shouldRefresh,
      );

      // Invalidate the specific artifact query (used by ProgressDetail, TextFileDetail)
      // This prefix includes the artifact's version queries.
      scheduleArtifactRefresh(
        queryClient,
        ["artifacts", updatedArtifactId],
        shouldRefresh,
      );

      // Invalidate the execution artifacts list query
      if (artifactExecution) {
        scheduleArtifactRefresh(
          queryClient,
          ["artifacts", "execution", artifactExecution],
          shouldRefresh,
        );
      }
    },
    [artifactId, executionId, paused, queryClient, shouldRefresh],
  );

  const { connected } = useEntityNotifications(
    "artifact",
    handleNotification,
    enabled && !paused,
  );

  return {
    isConnected: connected,
  };
}
