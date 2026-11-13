export const LOG_FINALIZING_MESSAGE =
  "Log is still being finalized. Try again shortly.";
export const LOG_INCOMPLETE_MESSAGE =
  "Log is incomplete because delivery did not finish.";
const CONTENT_UNAVAILABLE = "Content unavailable or access denied";

export class ArtifactDeliveryError extends Error {
  constructor(
    public readonly code: "LOG_FINALIZING" | "LOG_INCOMPLETE",
    public readonly retryAfterMs?: number,
  ) {
    super(
      code === "LOG_FINALIZING"
        ? LOG_FINALIZING_MESSAGE
        : LOG_INCOMPLETE_MESSAGE,
    );
  }
}

export function isArtifactFinalizing(
  error: unknown,
): error is ArtifactDeliveryError {
  return (
    error instanceof ArtifactDeliveryError && error.code === "LOG_FINALIZING"
  );
}

export function artifactContentErrorMessage(error: unknown): string {
  return error instanceof ArtifactDeliveryError
    ? error.message
    : CONTENT_UNAVAILABLE;
}

export function artifactStreamErrorMessage(data: string): string {
  try {
    const { code } = JSON.parse(data) as { code?: string };
    if (code === "log_stream_incomplete") return LOG_INCOMPLETE_MESSAGE;
  } catch {
    /* Legacy and unknown frames remain generic. */
  }
  return CONTENT_UNAVAILABLE;
}

/** Only known delivery codes may become UI text. Never display arbitrary error bodies. */
export async function artifactContentError(response: Response): Promise<Error> {
  let code: unknown;
  if (response.status === 409 && response.body) {
    const reader = response.body.getReader();
    let bytes = new Uint8Array(0);
    try {
      while (bytes.length <= 4096) {
        const { value, done } = await reader.read();
        if (done) {
          code = (
            JSON.parse(new TextDecoder().decode(bytes)) as { code?: unknown }
          ).code;
          break;
        }
        if (bytes.length + value.length > 4096) break;
        const next = new Uint8Array(bytes.length + value.length);
        next.set(bytes);
        next.set(value, bytes.length);
        bytes = next;
      }
    } catch {
      /* Unknown or interrupted error bodies remain generic. */
    } finally {
      try {
        await reader.cancel();
      } finally {
        reader.releaseLock();
      }
    }
  } else {
    await response.body?.cancel();
  }
  if (code === "LOG_FINALIZING")
    return new ArtifactDeliveryError(
      "LOG_FINALIZING",
      retryAfterMilliseconds(response.headers.get("Retry-After")),
    );
  if (code === "LOG_INCOMPLETE")
    return new ArtifactDeliveryError("LOG_INCOMPLETE");
  return new Error(`HTTP ${response.status}: ${response.statusText}`);
}

export const ARTIFACT_PREVIEW_BYTES = 64 * 1024;

export async function readArtifactPreview(response: Response) {
  if (
    response.status === 416 &&
    response.headers.get("Content-Range") === "bytes */0"
  ) {
    await response.body?.cancel();
    return { text: "", truncated: false };
  }
  if (!response.ok) {
    throw await artifactContentError(response);
  }
  if (!response.body) return { text: "", truncated: false };

  const range = response.headers.get("Content-Range");
  const match = range?.match(/^bytes 0-(\d+)\/(\d+)$/);
  const total = match ? Number(match[2]) : undefined;
  const reader = response.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let text = "";
  let bytes = 0;
  let done = false;
  let truncated = total !== undefined && total > ARTIFACT_PREVIEW_BYTES;

  try {
    while (bytes < ARTIFACT_PREVIEW_BYTES) {
      const chunk = await reader.read();
      if (chunk.done) {
        done = true;
        break;
      }
      const remaining = ARTIFACT_PREVIEW_BYTES - bytes;
      const bounded = chunk.value.subarray(0, remaining);
      text += decoder.decode(bounded, { stream: true });
      bytes += bounded.byteLength;
      if (chunk.value.byteLength > remaining) truncated = true;
    }
    if (!done && bytes === ARTIFACT_PREVIEW_BYTES && total === undefined) {
      truncated = true;
    }
    // An incomplete UTF-8 character at a truncated boundary is omitted, not
    // flushed into a replacement character or reported as a file error.
    if (!truncated) text += decoder.decode();
    return { text, truncated };
  } finally {
    try {
      if (!done) await reader.cancel();
    } finally {
      reader.releaseLock();
    }
  }
}

function retryAfterMilliseconds(value: string | null): number | undefined {
  if (value === null || !/^[0-9]+$/.test(value)) return undefined;
  const milliseconds = Number(value) * 1000;
  return Number.isSafeInteger(milliseconds)
    ? Math.min(milliseconds, 2000)
    : undefined;
}

function waitForDeliveryRetry(
  milliseconds: number,
  signal: AbortSignal,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const abort = () => {
      clearTimeout(timer);
      signal.removeEventListener("abort", abort);
      reject(signal.reason ?? new DOMException("Aborted", "AbortError"));
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, milliseconds);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
  });
}

/** Wait only for an authorized finalizing response for one immutable version.
 * A mutable latest selector and all other HTTP failures remain single attempts.
 */
type DeliveryWaitOptions = { onFinalizing?: () => void; maxWaitMs?: number };
export function fetchArtifactPreview(
  url: string,
  request: RequestInit & { signal: AbortSignal },
  options: DeliveryWaitOptions = {},
) {
  return waitForArtifactDelivery(url, request, readArtifactPreview, options);
}
export function fetchArtifactDownload(
  url: string,
  request: RequestInit & { signal: AbortSignal },
  options: DeliveryWaitOptions = {},
) {
  return waitForArtifactDelivery(
    url,
    request,
    async (response) => {
      if (!response.ok) throw await artifactContentError(response);
      // Consume the body while the request owns its cancellation lifetime.
      // Returning a Response here would abort an unread download in finally.
      return { headers: response.headers, blob: await response.blob() };
    },
    options,
  );
}
async function waitForArtifactDelivery<T>(
  url: string,
  request: RequestInit & { signal: AbortSignal },
  consume: (response: Response) => Promise<T>,
  options: DeliveryWaitOptions,
) {
  const maxWaitMs = options.maxWaitMs ?? 65_000;
  if (!Number.isFinite(maxWaitMs) || maxWaitMs <= 0 || maxWaitMs > 300_000)
    throw new Error("Invalid log availability wait budget");
  const version = new URL(url, "http://local.invalid").searchParams.get(
    "version_id",
  );
  const immutable = version !== null && /^[1-9][0-9]*$/.test(version);
  const controller = new AbortController();
  const abort = () => controller.abort(request.signal.reason);
  request.signal.addEventListener("abort", abort, { once: true });
  if (request.signal.aborted) abort();
  const timer = setTimeout(() => controller.abort(), maxWaitMs);
  let finalizing: ArtifactDeliveryError | undefined;
  try {
    for (let attempt = 0; ; attempt++) {
      controller.signal.throwIfAborted();
      try {
        const response = await fetch(url, {
          ...request,
          signal: controller.signal,
        });
        // The availability budget ends when content is available. Large
        // downloads may take longer; caller cancellation still owns the body.
        if (response.ok) clearTimeout(timer);
        return await consume(response);
      } catch (error) {
        if (!immutable || !isArtifactFinalizing(error)) throw error;
        finalizing = error;
        options.onFinalizing?.();
        const backoff = Math.min(500 * 2 ** Math.min(attempt, 3), 2000);
        const delay =
          error.retryAfterMs ??
          Math.round(backoff * (0.8 + Math.random() * 0.2));
        await waitForDeliveryRetry(Math.max(500, delay), controller.signal);
      }
    }
  } catch (error) {
    // An exhausted client wait is still pending, not evidence of lost logs.
    if (controller.signal.aborted && !request.signal.aborted && finalizing)
      throw finalizing;
    throw error;
  } finally {
    clearTimeout(timer);
    request.signal.removeEventListener("abort", abort);
    controller.abort();
  }
}
