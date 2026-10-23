import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AbsentMetadataPolicy } from "@/api";
import { useInstallPack, usePackInstallStatus } from "@/hooks/usePackTests";

const installPack = vi.fn();
const getPackInstall = vi.fn();

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return {
    ...actual,
    PacksService: {
      ...actual.PacksService,
      installPack: (...args: unknown[]) => installPack(...args),
      getPackInstall: (...args: unknown[]) => getPackInstall(...args),
    },
  };
});

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { mutations: { retry: false }, queries: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

beforeEach(() => {
  installPack.mockReset();
  getPackInstall.mockReset();
  installPack.mockResolvedValue({ data: { pack: { ref: "core" } } });
});

it("reports failures when a known install attempt cannot be read", async () => {
  const error = { body: { error: "Install record unavailable" }, status: 404 };
  getPackInstall.mockRejectedValueOnce(error);
  const { result } = renderHook(() => usePackInstallStatus("demo", 42, true), {
    wrapper: createWrapper(),
  });
  await waitFor(() => expect(result.current.isError).toBe(true));
  expect(result.current.error).toBe(error);
  expect(getPackInstall).toHaveBeenCalledWith({ id: 42 });
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
