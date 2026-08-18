import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { PacksService, ApiError } from "@/api";

// Fetch test history for a pack
export function usePackTestHistory(
  packRef: string,
  params?: { page?: number; pageSize?: number },
) {
  return useQuery({
    queryKey: ["pack-tests", packRef, params],
    queryFn: async () => {
      return PacksService.getPackTestHistory({
        ref: packRef,
        page: params?.page,
        pageSize: params?.pageSize,
      });
    },
    enabled: !!packRef,
    staleTime: 30000, // 30 seconds
  });
}

// Fetch a single pack test execution by id
export function usePackTest(packId: number | undefined) {
  return useQuery({
    queryKey: ["pack-test", packId],
    queryFn: async () => {
      return PacksService.getPackTest({ id: packId as number });
    },
    enabled: !!packId,
  });
}

// Fetch latest test result for a pack
export function usePackLatestTest(packRef: string) {
  return useQuery({
    queryKey: ["pack-tests", packRef, "latest"],
    queryFn: async () => {
      try {
        return await PacksService.getPackLatestTest({ ref: packRef });
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) {
          return { data: null };
        }
        throw error;
      }
    },
    enabled: !!packRef,
    staleTime: 30000,
  });
}

// Poll the latest pack install status. Enabled while a worker-completed test
// run is in flight (pending/running); refetches every 2 seconds until terminal.
export function usePackInstallStatus(packRef: string | undefined, enabled = false) {
  return useQuery({
    queryKey: ["pack-install", packRef],
    queryFn: async () => {
      try {
        return await PacksService.getPackLatestInstall({ ref: packRef as string });
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) {
          return { data: null };
        }
        throw error;
      }
    },
    enabled: !!packRef && enabled,
    refetchInterval: (query) => {
      const status = query.state.data?.data?.status;
      return status === "pending" || status === "running" ? 2000 : false;
    },
  });
}

// Execute pack tests (dispatched to a worker)
export function useExecutePackTests() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async (packRef: string) => {
      return PacksService.testPack({ ref: packRef });
    },
    onSuccess: (_data, packRef) => {
      // Invalidate test history and install status queries
      queryClient.invalidateQueries({ queryKey: ["pack-tests", packRef] });
      queryClient.invalidateQueries({ queryKey: ["pack-install", packRef] });
    },
  });
}

// Install pack from remote source
export function useInstallPack() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: async ({
      source,
      refSpec,
      skipTests = false,
      skipDeps = false,
    }: {
      source: string;
      refSpec?: string;
      skipTests?: boolean;
      skipDeps?: boolean;
    }) => {
      return PacksService.installPack({
        requestBody: {
          source,
          ref_spec: refSpec,
          skip_tests: skipTests,
          skip_deps: skipDeps,
        },
      });
    },
    onSuccess: (data) => {
      // Invalidate packs list and test queries
      queryClient.invalidateQueries({ queryKey: ["packs"] });
      if (data.data.pack.ref) {
        queryClient.invalidateQueries({
          queryKey: ["pack-tests", data.data.pack.ref],
        });
        queryClient.invalidateQueries({
          queryKey: ["pack-install", data.data.pack.ref],
        });
      }
    },
  });
}