import { beforeEach, expect, it, vi } from "vitest";
const post = vi.hoisted(() => vi.fn());
vi.mock("axios", () => ({ default: { create: () => ({ post }) } }));
import { refreshSession } from "./session-refresh";
beforeEach(() => {
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    clear: () => values.clear(),
  });
  post.mockReset();
});
it("shares one rotation across concurrent API and proactive callers", async () => {
  localStorage.setItem("refresh_token", "old");
  post.mockResolvedValue({
    data: { data: { access_token: "access", refresh_token: "next" } },
  });
  expect(await Promise.all([refreshSession(), refreshSession()])).toEqual([
    "access",
    "access",
  ]);
  expect(post).toHaveBeenCalledTimes(1);
  expect(localStorage.getItem("refresh_token")).toBe("next");
});
it("does not restore a session cleared during refresh", async () => {
  localStorage.setItem("refresh_token", "old");
  let resolve!: (v: unknown) => void;
  post.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const pending = refreshSession();
  // In environments with Web Locks the request begins asynchronously.
  await vi.waitFor(() => expect(post).toHaveBeenCalledTimes(1));
  localStorage.clear();
  resolve({
    data: { data: { access_token: "access", refresh_token: "next" } },
  });
  await expect(pending).rejects.toThrow("Session changed");
  expect(localStorage.getItem("refresh_token")).toBeNull();
});
