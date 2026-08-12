import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useKey } from "@/hooks/useKeys";

const getKey = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    SecretsService: {
      getKey: (...args: unknown[]) => getKey(...args),
    },
  };
});

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
  getKey.mockReset();
  getKey.mockResolvedValue({ data: { value: null } });
});

describe("useKey", () => {
  it("does not request decryption by default", async () => {
    const { result } = renderHook(() => useKey("system.token"), {
      wrapper: createWrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(getKey).toHaveBeenCalledWith({
      ref: "system.token",
      decrypt: false,
    });
  });

  it("gates explicit decryption on the enabled option", async () => {
    const { result, rerender } = renderHook(
      ({ enabled }) => useKey("system.token", { decrypt: true, enabled }),
      { initialProps: { enabled: false }, wrapper: createWrapper() },
    );

    expect(result.current.fetchStatus).toBe("idle");
    expect(getKey).not.toHaveBeenCalled();

    rerender({ enabled: true });
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(getKey).toHaveBeenCalledWith({
      ref: "system.token",
      decrypt: true,
    });
  });
});
