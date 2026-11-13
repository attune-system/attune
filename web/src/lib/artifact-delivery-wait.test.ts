import { afterEach, describe, expect, it, vi } from "vitest";
import {
  fetchArtifactDownload,
  fetchArtifactPreview,
  isArtifactFinalizing,
} from "./artifact-preview";
const exact =
  "http://local/api/v1/artifacts/42/versions/7/download?version_id=71";
const pending = () =>
  new Response(JSON.stringify({ code: "LOG_FINALIZING" }), {
    status: 409,
    headers: { "Retry-After": "1" },
  });
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
describe("waiting for an immutable log version", () => {
  it.each([false, true])(
    "keeps the streamed download alive after availability (delayed=%s)",
    async (delayed) => {
      vi.useFakeTimers();
      let body!: ReadableStreamDefaultController<Uint8Array>;
      let requestSignal!: AbortSignal;
      const fetch = vi.fn().mockImplementation((_url, request: RequestInit) => {
        if (delayed && fetch.mock.calls.length === 1)
          return Promise.resolve(pending());
        requestSignal = request.signal!;
        const stream = new ReadableStream<Uint8Array>({
          start(controller) {
            body = controller;
            controller.enqueue(new TextEncoder().encode("first "));
            requestSignal.addEventListener("abort", () => {
              controller.error(new DOMException("Aborted", "AbortError"));
            });
          },
        });
        return Promise.resolve(
          new Response(stream, {
            headers: {
              "Content-Disposition": 'attachment; filename="log.txt"',
            },
          }),
        );
      });
      vi.stubGlobal("fetch", fetch);
      const work = fetchArtifactDownload(
        exact,
        { signal: new AbortController().signal },
        { maxWaitMs: 2500 },
      );
      // Fetch headers have arrived, but the response body is still incomplete.
      await vi.advanceTimersByTimeAsync(2501);
      expect(fetch).toHaveBeenCalledTimes(delayed ? 2 : 1);
      expect(requestSignal.aborted).toBe(false);
      body.enqueue(new TextEncoder().encode("last"));
      body.close();
      const download = await work;
      expect(await download.blob.text()).toBe("first last");
      expect(download.headers.get("Content-Disposition")).toContain("log.txt");
      expect(requestSignal.aborted).toBe(true);
      expect(vi.getTimerCount()).toBe(0);
    },
  );
  it("retries finalizing for the same version and returns its eventual content", async () => {
    vi.useFakeTimers();
    const fetch = vi
      .fn()
      .mockImplementationOnce(pending)
      .mockResolvedValueOnce(new Response("ready"));
    vi.stubGlobal("fetch", fetch);
    const onFinalizing = vi.fn();
    const work = fetchArtifactPreview(
      exact,
      { signal: new AbortController().signal },
      { onFinalizing },
    );
    await vi.advanceTimersByTimeAsync(1000);
    await expect(work).resolves.toEqual({ text: "ready", truncated: false });
    expect(onFinalizing).toHaveBeenCalledOnce();
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(fetch.mock.calls.every(([url]) => url === exact)).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });
  it.each([401, 403, 404, 500])("does not retry HTTP %s", async (status) => {
    const fetch = vi
      .fn()
      .mockResolvedValue(new Response("private text", { status }));
    vi.stubGlobal("fetch", fetch);
    await expect(
      fetchArtifactPreview(exact, { signal: new AbortController().signal }),
    ).rejects.toThrow(`HTTP ${status}`);
    expect(fetch).toHaveBeenCalledOnce();
  });
  it("does not follow a mutable latest selector into another execution", async () => {
    const fetch = vi.fn().mockImplementation(pending);
    vi.stubGlobal("fetch", fetch);
    const error = await fetchArtifactPreview(
      "http://local/api/v1/artifacts/42/download",
      { signal: new AbortController().signal },
    ).catch((e) => e);
    expect(isArtifactFinalizing(error)).toBe(true);
    expect(fetch).toHaveBeenCalledOnce();
  });
  it("stops retries and removes timers when the viewer is closed", async () => {
    vi.useFakeTimers();
    const fetch = vi.fn().mockImplementation(pending);
    vi.stubGlobal("fetch", fetch);
    const controller = new AbortController();
    const work = fetchArtifactPreview(exact, {
      signal: controller.signal,
    }).catch((error) => error);
    await vi.advanceTimersByTimeAsync(0);
    controller.abort();
    expect((await work).name).toBe("AbortError");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(fetch).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });
  it("retains finalizing rather than inventing incomplete when its wait budget expires", async () => {
    vi.useFakeTimers();
    const fetch = vi.fn().mockImplementation(pending);
    vi.stubGlobal("fetch", fetch);
    const work = fetchArtifactPreview(
      exact,
      { signal: new AbortController().signal },
      { maxWaitMs: 2500 },
    ).catch((e) => e);
    await vi.advanceTimersByTimeAsync(2500);
    expect(isArtifactFinalizing(await work)).toBe(true);
    expect(fetch).toHaveBeenCalledTimes(3);
    expect(vi.getTimerCount()).toBe(0);
  });
  it("stops on a definitive incomplete response", async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ code: "LOG_INCOMPLETE" }), {
        status: 409,
      }),
    );
    vi.stubGlobal("fetch", fetch);
    const error = await fetchArtifactPreview(exact, {
      signal: new AbortController().signal,
    }).catch((e) => e);
    expect(isArtifactFinalizing(error)).toBe(false);
    expect(error.code).toBe("LOG_INCOMPLETE");
    expect(fetch).toHaveBeenCalledOnce();
  });
});
