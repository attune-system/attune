import type { CancelablePromise } from "@/api/core/CancelablePromise";

export async function artifactRequest<T>(
  request: CancelablePromise<T>,
  signal: AbortSignal,
): Promise<T> {
  const cancel = () => request.cancel();
  if (signal.aborted) cancel();
  else signal.addEventListener("abort", cancel, { once: true });
  try {
    return await request;
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}
