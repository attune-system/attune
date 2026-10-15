import { describe, expect, it, vi } from "vitest";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import { artifactRequest } from "./artifact-request";

describe("artifact query cancellation", () => {
  it("cancels the generated request when React Query aborts", async () => {
    const cancel = vi.fn();
    const controller = new AbortController();
    const request = new CancelablePromise<string>(
      (_resolve, _reject, onCancel) => {
        onCancel(cancel);
      },
    );
    const result = artifactRequest(request, controller.signal);
    controller.abort();
    await expect(result).rejects.toThrow("Request aborted");
    expect(cancel).toHaveBeenCalledOnce();
  });

  it("handles an already aborted query", async () => {
    const controller = new AbortController();
    controller.abort();
    const request = new CancelablePromise<string>(() => {});
    await expect(artifactRequest(request, controller.signal)).rejects.toThrow(
      "Request aborted",
    );
  });

  it("removes cancellation subscriptions after settlement", async () => {
    const controller = new AbortController();
    const cancel = vi.fn();
    const request = new CancelablePromise<string>(
      (resolve, _reject, onCancel) => {
        onCancel(cancel);
        resolve("complete");
      },
    );
    await expect(artifactRequest(request, controller.signal)).resolves.toBe(
      "complete",
    );
    controller.abort();
    expect(cancel).not.toHaveBeenCalled();
  });
});
