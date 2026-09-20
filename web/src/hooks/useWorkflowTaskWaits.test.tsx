import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkflowTaskWaitKind, WorkflowTaskWaitState } from "@/api";
import {
  shouldPollWorkflowTaskWaits,
  useWorkflowTaskWaits,
  type WorkflowTaskWait,
} from "./useWorkflowTaskWaits";

const listWorkflowTaskWaits = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    ExecutionsService: {
      ...actual.ExecutionsService,
      listWorkflowTaskWaits: (...args: unknown[]) =>
        listWorkflowTaskWaits(...args),
    },
  };
});

const waiting: WorkflowTaskWait = {
  id: 1,
  target_id: 2,
  kind: WorkflowTaskWaitKind.INQUIRY,
  state: WorkflowTaskWaitState.WAITING,
  task_name: "deploy",
  created: "2026-08-05T10:00:00Z",
  updated: "2026-08-05T10:00:00Z",
};

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  listWorkflowTaskWaits.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("useWorkflowTaskWaits", () => {
  it("calls the generated service and polls a waiting result", async () => {
    listWorkflowTaskWaits.mockResolvedValue({ data: [waiting] });
    renderHook(() => useWorkflowTaskWaits(42, "running"), {
      wrapper: createWrapper(),
    });

    await vi.waitFor(() => {
      expect(listWorkflowTaskWaits).toHaveBeenCalledWith({ id: 42 });
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    await vi.waitFor(() => {
      expect(listWorkflowTaskWaits).toHaveBeenCalledTimes(2);
    });
  });

  it("polls only while the parent and a wait can still change", () => {
    expect(shouldPollWorkflowTaskWaits("running", [])).toBe(true);
    expect(
      shouldPollWorkflowTaskWaits("completed", [
        { ...waiting, state: WorkflowTaskWaitState.RELEASED },
      ]),
    ).toBe(false);
    expect(
      shouldPollWorkflowTaskWaits("completed", [
        { ...waiting, state: WorkflowTaskWaitState.FAILED },
      ]),
    ).toBe(false);
  });

  it.each(["inquiry", "execution", "work_queue_item"] as const)(
    "keeps polling a stale %s wait after the parent becomes terminal",
    (kind) => {
      expect(
        shouldPollWorkflowTaskWaits("completed", [{ ...waiting, kind }]),
      ).toBe(true);
    },
  );

  it("keeps generalized target metadata after a refetch", async () => {
    const executionWait: WorkflowTaskWait = {
      ...waiting,
      kind: "execution",
      target_id: 73,
    };
    const queueWait: WorkflowTaskWait = {
      ...waiting,
      id: 2,
      kind: "work_queue_item",
      target_id: 84,
      work_queue_ref: "ops.deployments",
    };
    listWorkflowTaskWaits
      .mockResolvedValueOnce({ data: [executionWait] })
      .mockResolvedValueOnce({ data: [queueWait] });

    const { result } = renderHook(() => useWorkflowTaskWaits(42, "completed"), {
      wrapper: createWrapper(),
    });
    await vi.waitFor(() =>
      expect(result.current.data?.data).toEqual([executionWait]),
    );

    const refreshed = await act(() => result.current.refetch());
    expect(refreshed.data?.data).toEqual([queueWait]);
  });
});
