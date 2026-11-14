import { beforeEach, expect, it, vi } from "vitest";
const post = vi.hoisted(() => vi.fn());
vi.mock("axios", () => ({ default: { create: () => ({ post }) } }));
import { refreshSession, revokeBrowserSession } from "./session-refresh";
beforeEach(() => {
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    clear: () => values.clear(),
  });
  vi.stubGlobal("navigator", {
    locks: {
      request: async (_name: string, callback: () => Promise<string>) =>
        await callback(),
    },
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

it("fails closed across independent tabs without Web Locks before sending either refresh", async () => {
  vi.stubGlobal("navigator", {});
  localStorage.setItem("refresh_token", "old");
  vi.resetModules();
  const tab1 = await import("./session-refresh");
  vi.resetModules();
  const tab2 = await import("./session-refresh");
  const results = await Promise.allSettled([
    tab1.refreshSession(),
    tab2.refreshSession(),
  ]);
  expect(results.map((r) => r.status)).toEqual(["rejected", "rejected"]);
  expect(post).not.toHaveBeenCalled();
});
it("uses the configured API origin for logout and retains credentials on failure", async () => {
  vi.stubEnv("VITE_API_BASE_URL", "https://api.example.test");
  localStorage.setItem("refresh_token", "old");
  post.mockRejectedValue(new Error("offline"));
  await expect(revokeBrowserSession()).rejects.toThrow("retry signing out");
  expect(post).toHaveBeenCalledWith(
    "https://api.example.test/auth/session/logout",
    { refresh_token: "old" },
  );
  expect(localStorage.getItem("refresh_token")).toBe("old");
  vi.unstubAllEnvs();
});
