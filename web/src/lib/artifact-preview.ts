export const LOG_FINALIZING_MESSAGE =
  "Log is still being finalized. Try again shortly.";
export const LOG_INCOMPLETE_MESSAGE =
  "Log is incomplete because delivery did not finish.";
const CONTENT_UNAVAILABLE = "Content unavailable or access denied";

class ArtifactDeliveryError extends Error {}

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
    return new ArtifactDeliveryError(LOG_FINALIZING_MESSAGE);
  if (code === "LOG_INCOMPLETE")
    return new ArtifactDeliveryError(LOG_INCOMPLETE_MESSAGE);
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
