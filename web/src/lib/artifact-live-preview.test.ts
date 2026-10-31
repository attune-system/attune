import { describe, expect, it, vi } from "vitest";
import {
  appendArtifactPreviewTail,
  ArtifactSseDecoder,
  ArtifactSseProtocolError,
  ARTIFACT_SSE_FRAME_BYTES,
} from "./artifact-live-preview";

describe("bounded artifact preview tail", () => {
  it("preserves a literal BOM at the displayed text boundary", () => {
    expect(
      appendArtifactPreviewTail({ text: "", truncated: false }, "\ufeff \t\n")
        .text,
    ).toBe("\ufeff \t\n");
  });
  it("keeps exactly 64 KiB without claiming truncation until an older prefix is dropped", () => {
    let tail = appendArtifactPreviewTail(
      { text: "", truncated: false },
      "x".repeat(65536),
    );
    expect(tail.text.length).toBe(65536);
    expect(tail.truncated).toBe(false);
    tail = appendArtifactPreviewTail(tail, "😀");
    expect(tail.text.length).toBe(65534);
    expect(tail.text.endsWith("😀")).toBe(true);
    expect(tail.truncated).toBe(true);
    expect(appendArtifactPreviewTail(tail, "").truncated).toBe(true);
    expect(appendArtifactPreviewTail(tail, " \tfresh\n", true)).toEqual({
      text: " \tfresh\n",
      truncated: false,
    });
  });
});

describe("bounded execution SSE decoding", () => {
  const encode = (text: string) => new TextEncoder().encode(text);

  it.each(["\n", "\r", "\r\n"])(
    "preserves split UTF-8, whitespace, and complete u64 fields with %j separators",
    (separator) => {
      const block = [
        "id: 18446744073709551615",
        "event: content",
        "data:   😀界\t",
        "data: next",
        "",
        "",
      ].join(separator);
      const consume = vi.fn<(block: string) => boolean>(() => true);
      const decoder = new ArtifactSseDecoder();
      for (const byte of encode(block))
        decoder.push(new Uint8Array([byte]), consume);
      expect(consume).toHaveBeenCalledOnce();
      expect(
        consume.mock.calls[0][0].split(/\r\n|\r|\n/).filter(Boolean),
      ).toEqual([
        "id: 18446744073709551615",
        "event: content",
        "data:   😀界\t",
        "data: next",
      ]);
    },
  );

  it.each([16387, 1024 * 1024])(
    "accepts the worst newline expansion of a %i-byte server chunk",
    (size) => {
      // Protected frames + UTF-8 carry, or legacy 1 MiB reads, every byte a
      // newline. Axum emits a separate `data: ` prefix after every delimiter.
      const block = `id: 18446744073709551615\nevent: append\n${"data: \n".repeat(size + 1)}\n`;
      expect(encode(block).length).toBeLessThan(ARTIFACT_SSE_FRAME_BYTES);
      const consume = vi.fn(() => true);
      new ArtifactSseDecoder().push(encode(block), consume);
      expect(consume).toHaveBeenCalledExactlyOnceWith(block);
    },
  );

  it("rejects an oversized unterminated frame across reads without synthesizing an event", () => {
    const consume = vi.fn(() => true);
    const decoder = new ArtifactSseDecoder();
    const chunk = encode("x".repeat(16384));
    for (let index = 0; index < 512; index++) decoder.push(chunk, consume);
    expect(() => decoder.push(encode("x"), consume)).toThrow(
      ArtifactSseProtocolError,
    );
    expect(consume).not.toHaveBeenCalled();
  });

  it("stops at a terminal or denied event even when another event shares the transport read", () => {
    const consume = vi.fn(() => false);
    new ArtifactSseDecoder().push(
      encode(
        "event: error\ndata: denied\n\nevent: append\ndata: forbidden\n\n",
      ),
      consume,
    );
    expect(consume).toHaveBeenCalledExactlyOnceWith(
      "event: error\ndata: denied\n\n",
    );
  });
});
