import { describe, expect, it, vi } from "vitest";
import {
  artifactContentErrorMessage,
  artifactStreamErrorMessage,
  LOG_FINALIZING_MESSAGE,
  LOG_INCOMPLETE_MESSAGE,
  ARTIFACT_PREVIEW_BYTES,
  readArtifactPreview,
} from "./artifact-preview";

describe("artifact previews", () => {
  it.each([
    ["LOG_FINALIZING", LOG_FINALIZING_MESSAGE],
    ["LOG_INCOMPLETE", LOG_INCOMPLETE_MESSAGE],
  ])("distinguishes authorized %s diagnostics", async (code, message) => {
    const response = new Response(
      JSON.stringify({ code, error: "untrusted server text" }),
      { status: 409 },
    );
    try {
      await readArtifactPreview(response);
      throw new Error("expected failure");
    } catch (error) {
      expect(artifactContentErrorMessage(error)).toBe(message);
    }
  });

  it.each([403, 404, 409, 500])(
    "keeps unknown and unauthorized HTTP %s errors generic",
    async (status) => {
      const response = new Response(
        JSON.stringify({ code: "UNKNOWN", error: "private information" }),
        { status },
      );
      try {
        await readArtifactPreview(response);
        throw new Error("expected failure");
      } catch (error) {
        expect(artifactContentErrorMessage(error)).toBe(
          "Content unavailable or access denied",
        );
      }
    },
  );

  it("does not accept delivery codes from authorization failures", async () => {
    const response = new Response(JSON.stringify({ code: "LOG_INCOMPLETE" }), {
      status: 403,
    });
    await expect(
      readArtifactPreview(response).catch(artifactContentErrorMessage),
    ).resolves.toBe("Content unavailable or access denied");
  });

  it("bounds untrusted delivery error bodies", async () => {
    const response = new Response(
      JSON.stringify({ padding: "x".repeat(8192), code: "LOG_INCOMPLETE" }),
      { status: 409 },
    );
    await expect(
      readArtifactPreview(response).catch(artifactContentErrorMessage),
    ).resolves.toBe("Content unavailable or access denied");
    expect(response.body?.locked).toBe(false);
  });

  it("shows incomplete SSE logs without accepting arbitrary error text", () => {
    expect(
      artifactStreamErrorMessage(
        JSON.stringify({ code: "log_stream_incomplete", message: "private" }),
      ),
    ).toBe(LOG_INCOMPLETE_MESSAGE);
    expect(
      artifactStreamErrorMessage(
        JSON.stringify({ code: "denied", message: "private" }),
      ),
    ).toBe("Content unavailable or access denied");
  });

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
