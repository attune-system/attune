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
    await response.body?.cancel();
    throw new Error(`HTTP ${response.status}: ${response.statusText}`);
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
