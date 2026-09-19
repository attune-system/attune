import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Notification,
  NotificationHandler,
} from "@/contexts/WebSocketContext";
import { useExecutionStream } from "./useExecutionStream";

let notificationHandler: NotificationHandler | undefined;

vi.mock("@/contexts/WebSocketContext", () => ({
  useEntityNotifications: (
    _entityType: string,
    handler: NotificationHandler,
  ) => {
    notificationHandler = handler;
    return { connected: true };
  },
}));

beforeEach(() => {
  notificationHandler = undefined;
});

describe("useExecutionStream", () => {
  it("refreshes execution details and artifacts when an execution becomes terminal", () => {
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const invalidateQueries = vi.spyOn(queryClient, "invalidateQueries");

    renderHook(() => useExecutionStream({ executionId: 42 }), {
      wrapper: ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={queryClient}>
          {children}
        </QueryClientProvider>
      ),
    });

    const notification: Notification = {
      notification_type: "execution_status_changed",
      entity_type: "execution",
      entity_id: 42,
      payload: { status: "completed", old_status: "running" },
      timestamp: "2026-09-16T17:30:37Z",
    };

    act(() => notificationHandler?.(notification));

    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: ["executions", 42],
      exact: true,
    });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: ["artifacts", "execution", 42],
      exact: true,
    });
  });
});
