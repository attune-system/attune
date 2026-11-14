import axios from "axios";

const refreshClient = axios.create({ timeout: 10000 });
let inFlight: Promise<string> | undefined;

/** All UI consumers share one rotation; Web Locks also serialize browser tabs. */
export function refreshSession(): Promise<string> {
  if (inFlight) return inFlight;
  const observed = localStorage.getItem("refresh_token");
  const rotate = async () => {
    const current = localStorage.getItem("refresh_token");
    if (!current) throw new Error("No refresh session");
    // Another tab already persisted the successor while this tab waited.
    if (current !== observed) return localStorage.getItem("access_token") || "";
    const base = import.meta.env.VITE_API_BASE_URL || "";
    const response = await refreshClient.post(`${base}/auth/refresh`, {
      refresh_token: current,
    });
    const tokens = response.data.data;
    // Logout or a new login must not be overwritten by an in-flight response.
    if (localStorage.getItem("refresh_token") !== current)
      throw new Error("Session changed during refresh");
    localStorage.setItem("access_token", tokens.access_token);
    localStorage.setItem("refresh_token", tokens.refresh_token);
    window.dispatchEvent(new CustomEvent("auth:token-changed"));
    return tokens.access_token as string;
  };
  const run = async (): Promise<string> => {
    if (navigator.locks)
      return await navigator.locks.request(
        "attune-interactive-refresh",
        rotate,
      );
    throw new Error(
      "Session refresh requires Web Locks. Use a supported browser over HTTPS and sign in again.",
    );
  };
  inFlight = run().finally(() => {
    inFlight = undefined;
  });
  return inFlight;
}

/** Fail without clearing local state when server revocation cannot be confirmed. */
export async function revokeBrowserSession(): Promise<void> {
  const refresh = localStorage.getItem("refresh_token");
  if (!refresh) return;
  const base = import.meta.env.VITE_API_BASE_URL || "";
  try {
    await refreshClient.post(`${base}/auth/session/logout`, {
      refresh_token: refresh,
    });
  } catch {
    throw new Error(
      "Session revocation failed. Check your connection and retry signing out.",
    );
  }
}
