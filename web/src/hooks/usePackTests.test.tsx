import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AbsentMetadataPolicy } from "@/api";
import { useInstallPack } from "@/hooks/usePackTests";

const installPack = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    PacksService: {
      ...actual.PacksService,
      installPack: (...args: unknown[]) => installPack(...args),
    },
  };
});

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

beforeEach(() => {
  installPack.mockReset();
  installPack.mockResolvedValue({ data: { pack: { ref: "core" } } });
});

describe("useInstallPack", () => {
  it("sends remove by default", async () => {
    const { result } = renderHook(() => useInstallPack(), {
      wrapper: createWrapper(),
    });

    await act(() => result.current.mutateAsync({ source: "core" }));

    expect(installPack).toHaveBeenCalledWith({
      requestBody: {
        source: "core",
        ref_spec: undefined,
        skip_tests: false,
        skip_deps: false,
        absent_metadata_policy: AbsentMetadataPolicy.REMOVE,
      },
    });
  });

  it("sends the selected policy", async () => {
    const { result } = renderHook(() => useInstallPack(), {
      wrapper: createWrapper(),
    });

    await act(() =>
      result.current.mutateAsync({
        source: "core",
        absentMetadataPolicy: AbsentMetadataPolicy.RETAIN,
      }),
    );

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(installPack).toHaveBeenCalledWith({
      requestBody: expect.objectContaining({
        absent_metadata_policy: AbsentMetadataPolicy.RETAIN,
      }),
    });
  });
});
