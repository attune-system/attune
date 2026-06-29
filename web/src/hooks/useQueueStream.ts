import { useCallback } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  useEntityNotifications,
  type Notification,
} from "@/contexts/WebSocketContext";
import { queueKeys } from "@/hooks/useQueues";

interface UseQueueStreamOptions {
  queueRef?: string;
  enabled?: boolean;
  paused?: boolean;
}

interface WorkQueueNotificationPayload {
  ref?: string;
  queue_ref?: string;
}

function resolveQueueRef(notification: Notification): string | undefined {
  const payload = notification.payload as
    WorkQueueNotificationPayload | undefined;
  if (notification.entity_type === "work_queue") {
    return payload?.ref;
  }
  return payload?.queue_ref;
}

export function useQueueStream(options: UseQueueStreamOptions = {}) {
  const { queueRef, enabled = true, paused = false } = options;
  const queryClient = useQueryClient();

  const handleNotification = useCallback(
    (notification: Notification) => {
      if (paused) {
        return;
      }

      const updatedQueueRef = resolveQueueRef(notification);
      if (queueRef && updatedQueueRef && updatedQueueRef !== queueRef) {
        return;
      }

      queryClient.invalidateQueries({ queryKey: queueKeys.all });

      if (updatedQueueRef) {
        queryClient.invalidateQueries({
          queryKey: queueKeys.detail(updatedQueueRef),
        });
        queryClient.invalidateQueries({
          queryKey: queueKeys.items(updatedQueueRef),
        });
      }
    },
    [paused, queryClient, queueRef],
  );

  const queueStream = useEntityNotifications(
    "work_queue",
    handleNotification,
    enabled && !paused,
  );
  const queueItemStream = useEntityNotifications(
    "work_queue_item",
    handleNotification,
    enabled && !paused,
  );

  return {
    isConnected: queueStream.connected || queueItemStream.connected,
  };
}
