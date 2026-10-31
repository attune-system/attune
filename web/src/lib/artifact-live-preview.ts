import { ARTIFACT_PREVIEW_BYTES } from "./artifact-preview";

export interface ArtifactPreviewTail {
  text: string;
  truncated: boolean;
}

/** Retain whole UTF-8 code points, never a partial leading character. */
export function appendArtifactPreviewTail(
  previous: ArtifactPreviewTail,
  text: string,
  replace = false,
): ArtifactPreviewTail {
  const bytes = new TextEncoder().encode((replace ? "" : previous.text) + text);
  let start = Math.max(0, bytes.length - ARTIFACT_PREVIEW_BYTES);
  while (start < bytes.length && (bytes[start] & 0xc0) === 0x80) start++;
  return {
    text: new TextDecoder("utf-8", { ignoreBOM: true }).decode(
      bytes.subarray(start),
    ),
    truncated: (!replace && previous.truncated) || start > 0,
  };
}

// executions.rs::LOG_STREAM_READ_CHUNK_SIZE permits 1 MiB legacy chunks for
// both shared files and object segments. Protected runtime_read_log frames are
// smaller (16 KiB, plus at most 3 UTF-8 carry bytes in log_decoder).
// String::from_utf8_lossy expands bad bytes up to 3x; Axum's CR/LF data prefixes
// expand newline bytes up to 7x. These are alternative per-byte worst cases,
// not multiplicative. 8 MiB covers 7 * 1 MiB + data/event/u64-ID framing.
// This bounds one pending wire event, not total browser/decoded-copy memory.
export const ARTIFACT_SSE_FRAME_BYTES = 8 * 1024 * 1024;

export class ArtifactSseProtocolError extends Error {}

/** Bound incomplete frames before decoding. Transport chunks can contain many
 * complete events and may split any UTF-8 character or CRLF delimiter. */
export class ArtifactSseDecoder {
  private readonly bytes = new Uint8Array(ARTIFACT_SSE_FRAME_BYTES);
  private readonly decoder = new TextDecoder("utf-8", {
    fatal: true,
    ignoreBOM: true,
  });
  private length = 0;
  private lineHasContent = false;
  private afterCr = false;

  push(chunk: Uint8Array, consume: (block: string) => boolean): void {
    for (const byte of chunk) {
      if (this.afterCr && byte === 10) {
        this.afterCr = false;
        // Keep CRLF wire bytes in the bound, except after dispatching a frame
        // ending with CR, where LF belongs to that already-consumed separator.
        if (this.length > 0) this.append(byte);
        continue;
      }
      this.afterCr = byte === 13;
      this.append(byte);
      if (byte !== 10 && byte !== 13) {
        this.lineHasContent = true;
        continue;
      }
      if (this.lineHasContent) {
        this.lineHasContent = false;
        continue;
      }
      let block: string;
      try {
        block = this.decoder.decode(this.bytes.subarray(0, this.length));
      } catch {
        throw new ArtifactSseProtocolError("Invalid execution log encoding");
      }
      this.length = 0;
      if (!consume(block)) return;
    }
  }

  private append(byte: number): void {
    if (this.length === this.bytes.length) {
      throw new ArtifactSseProtocolError(
        "Execution log frame exceeds its bound",
      );
    }
    this.bytes[this.length++] = byte;
  }
}
