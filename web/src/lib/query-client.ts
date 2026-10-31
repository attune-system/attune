import { QueryClient } from "@tanstack/react-query";

/** Generated requests expose status directly; Axios exposes response.status. */
export function httpErrorStatus(error: unknown): number | undefined {
  if (!error || typeof error !== "object") return undefined;
  if ("status" in error && typeof error.status === "number")
    return error.status;
  if (
    "response" in error &&
    error.response &&
    typeof error.response === "object" &&
    "status" in error.response &&
    typeof error.response.status === "number"
  ) {
    return error.response.status;
  }
  return undefined;
}

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        const status = httpErrorStatus(error);
        // No blind rate-limit retry: the user may retry deliberately. Only
        // transport errors and selected transient server errors get one retry.
        return (
          failureCount < 1 &&
          (status === undefined || [500, 502, 503, 504].includes(status))
        );
      },
      refetchOnWindowFocus: false,
      staleTime: 30000, // 30 seconds
      gcTime: 5 * 60 * 1000, // 5 minutes (formerly cacheTime)
    },
    mutations: {
      retry: 0,
    },
  },
});
