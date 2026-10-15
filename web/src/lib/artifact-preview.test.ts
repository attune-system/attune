import { describe, expect, it, vi } from "vitest";
import {
  ARTIFACT_PREVIEW_BYTES,
  readArtifactPreview,
} from "./artifact-preview";

describe("artifact previews", () => {
  it("reads complete small text", async () => {
    expect(await readArtifactPreview(new Response("hello 🐘"))).toEqual({
      text: "hello 🐘",
      truncated: false,
    });
  });

  it("omits a partial UTF-8 character at the range boundary", async () => {
    const bytes = new TextEncoder().encode(
      "x".repeat(ARTIFACT_PREVIEW_BYTES - 1) + "🐘",
    );
    const response = new Response(bytes.subarray(0, ARTIFACT_PREVIEW_BYTES), {
      status: 206,
      headers: {
        "Content-Range": `bytes 0-${ARTIFACT_PREVIEW_BYTES - 1}/${bytes.length}`,
      },
    });
    const preview = await readArtifactPreview(response);
    expect(preview.truncated).toBe(true);
    expect(preview.text).toBe("x".repeat(ARTIFACT_PREVIEW_BYTES - 1));
  });

  it("cancels an ignored range without retaining or decoding an oversized chunk", async () => {
    const cancel = vi.fn();
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(
          new TextEncoder().encode("x".repeat(ARTIFACT_PREVIEW_BYTES * 4)),
        );
      },
      cancel,
    });
    const preview = await readArtifactPreview(new Response(stream));
    expect(preview).toEqual({
      text: "x".repeat(ARTIFACT_PREVIEW_BYTES),
      truncated: true,
    });
    expect(cancel).toHaveBeenCalledOnce();
    expect(stream.locked).toBe(false);
  });

  it("decodes UTF-8 split across chunks", async () => {
    const bytes = new TextEncoder().encode("🐘");
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes.subarray(0, 2));
        controller.enqueue(bytes.subarray(2));
        controller.close();
      },
    });
    expect(await readArtifactPreview(new Response(stream))).toEqual({
      text: "🐘",
      truncated: false,
    });
  });

  it("accepts an empty file's unsatisfiable range", async () => {
    expect(
      await readArtifactPreview(
        new Response(null, {
          status: 416,
          headers: { "Content-Range": "bytes */0" },
        }),
      ),
    ).toEqual({ text: "", truncated: false });
  });

  it("does not disguise download authorization failures as empty previews", async () => {
    await expect(
      readArtifactPreview(new Response("denied", { status: 404 })),
    ).rejects.toThrow("HTTP 404");
  });

  it("releases the reader when the body fails", async () => {
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.error(new Error("request aborted"));
      },
    });
    await expect(readArtifactPreview(new Response(stream))).rejects.toThrow(
      "request aborted",
    );
    expect(stream.locked).toBe(false);
  });
});
