import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import axios from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useArtifact } from "@/hooks/useArtifacts";
import { ApiError } from "@/api/core/ApiError";
import { queryClient } from "./query-client";

vi.mock("@/contexts/WebSocketContext", () => ({
  useWebSocketContext: () => ({ connected: true }),
}));

describe("production artifact request retry policy", () => {
  afterEach(() => vi.restoreAllMocks());

  it.each([400, 401, 403, 404, 429])(
    "renders HTTP %s without a delayed duplicate request",
    async (status) => {
      // Exercise the generated request's ApiError, not a mocked hook/request.
      const send = vi.spyOn(axios, "request").mockResolvedValue({
        status,
        statusText: "Denied",
        data: {},
        headers: {},
      });
      const client = new QueryClient({
        defaultOptions: queryClient.getDefaultOptions(),
      });
      const wrapper = ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      );
      const hook = renderHook(() => useArtifact(42), { wrapper });
      try {
        await waitFor(() => expect(hook.result.current.isError).toBe(true), {
          timeout: 500,
        });
        expect(hook.result.current.error).toBeInstanceOf(ApiError);
        expect(send).toHaveBeenCalledOnce();
      } finally {
        hook.unmount();
        client.clear();
      }
    },
  );

  it.each(["network", 503])(
    "retries %s once, then exposes the failure",
    async (failure) => {
      const send = vi.spyOn(axios, "request");
      if (failure === "network")
        send.mockRejectedValue(new Error("connection lost"));
      else send.mockResolvedValue({ status: failure, data: {}, headers: {} });
      const client = new QueryClient({
        defaultOptions: queryClient.getDefaultOptions(),
      });
      const wrapper = ({ children }: { children: ReactNode }) => (
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      );
      const hook = renderHook(() => useArtifact(42), { wrapper });
      try {
        await waitFor(() => expect(hook.result.current.isError).toBe(true), {
          timeout: 2000,
        });
        expect(send).toHaveBeenCalledTimes(2);
      } finally {
        hook.unmount();
        client.clear();
      }
    },
  );

  it.each([400, 401, 403, 404, 429])(
    "does not retry Axios HTTP %s either",
    async (status) => {
      const client = new QueryClient({
        defaultOptions: queryClient.getDefaultOptions(),
      });
      const request = vi
        .fn()
        .mockRejectedValue(
          Object.assign(new Error("Denied"), { response: { status } }),
        );
      try {
        await expect(
          client.fetchQuery({ queryKey: ["axios", status], queryFn: request }),
        ).rejects.toThrow("Denied");
        expect(request).toHaveBeenCalledOnce();
      } finally {
        client.clear();
      }
    },
  );

  it("cancels a generated artifact request when its last observer unmounts", async () => {
    const send = vi
      .spyOn(axios, "request")
      .mockImplementation(() => new Promise(() => {}));
    const client = new QueryClient({
      defaultOptions: queryClient.getDefaultOptions(),
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const hook = renderHook(() => useArtifact(42), { wrapper });
    try {
      await waitFor(() => expect(send).toHaveBeenCalledOnce());
      const token = send.mock.calls[0][0].cancelToken;
      hook.unmount();
      await waitFor(() => expect(token?.reason).toBeDefined());
      expect(client.getQueryState(["artifacts", 42])?.fetchStatus).toBe("idle");
      expect(send).toHaveBeenCalledOnce();
    } finally {
      hook.unmount();
      client.clear();
    }
  });
});
