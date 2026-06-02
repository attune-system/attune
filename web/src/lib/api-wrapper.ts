import axios, { type InternalAxiosRequestConfig } from "axios";

/**
 * This module configures the generated API client to properly handle token refresh
 * and authentication errors.
 *
 * Key features:
 * - Automatic token refresh when access token expires (via axios interceptor)
 * - Proactive token refresh before expiration (every 60 seconds check)
 * - Proper redirect to login on authentication failure
 * - Distinguishes between 401 (expired/invalid token) and 403 (insufficient permissions)
 *
 * Strategy:
 * Since the generated API client creates its own axios instances, we configure
 * axios defaults globally and ensure the OpenAPI client uses our configured instance.
 *
 * IMPORTANT: All refresh calls use `refreshClient` — a bare axios instance with
 * NO interceptors — to prevent infinite 401 retry loops when the refresh token
 * itself is expired or invalid.
 */

const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || "";

// A bare axios instance with NO interceptors, used exclusively for token refresh
// requests. This prevents infinite loops when the refresh endpoint returns 401.
const refreshClient = axios.create({
  baseURL: API_BASE_URL || undefined,
  timeout: 10000,
  headers: { "Content-Type": "application/json" },
});

function getRefreshUrl(): string {
  return API_BASE_URL ? `${API_BASE_URL}/auth/refresh` : "/auth/refresh";
}

// Clear auth state and redirect to the login page.
// Safe to call multiple times — only the first redirect takes effect.
function clearSessionAndRedirect(): void {
  localStorage.removeItem("access_token");
  localStorage.removeItem("refresh_token");

  const currentPath = window.location.pathname;
  if (currentPath !== "/login" && currentPath !== "/auth/logout") {
    sessionStorage.setItem("redirect_after_login", currentPath);
  }

  if (currentPath !== "/auth/logout") {
    window.location.href = "/auth/logout";
  }
}

// Helper to decode JWT and check if it's expired or about to expire
export function isTokenExpiringSoon(
  token: string,
  thresholdSeconds: number = 300,
): boolean {
  try {
    const parts = token.split(".");
    if (parts.length !== 3) return true;

    const payload = JSON.parse(atob(parts[1]));
    const exp = payload.exp;

    if (!exp) return true;

    const now = Math.floor(Date.now() / 1000);
    const timeUntilExpiry = exp - now;

    // Return true if token expires within threshold seconds (default 5 minutes)
    return timeUntilExpiry <= thresholdSeconds;
  } catch {
    return true;
  }
}

// Helper to check if token is completely expired (not just expiring soon)
export function isTokenExpired(token: string): boolean {
  try {
    const parts = token.split(".");
    if (parts.length !== 3) return true;

    const payload = JSON.parse(atob(parts[1]));
    const exp = payload.exp;

    if (!exp) return true;

    const now = Math.floor(Date.now() / 1000);
    return exp <= now;
  } catch {
    return true;
  }
}

// Attempt to refresh the access token using the refresh token.
// Returns true on success, false on failure.
// On failure, clears session and redirects to login.
async function attemptTokenRefresh(): Promise<boolean> {
  const currentRefreshToken = localStorage.getItem("refresh_token");
  if (!currentRefreshToken) {
    console.warn("No refresh token available, redirecting to login");
    clearSessionAndRedirect();
    return false;
  }

  try {
    const response = await refreshClient.post(getRefreshUrl(), {
      refresh_token: currentRefreshToken,
    });

    const { access_token, refresh_token: newRefreshToken } = response.data.data;

    localStorage.setItem("access_token", access_token);
    if (newRefreshToken) {
      localStorage.setItem("refresh_token", newRefreshToken);
    }
    // Notify subscribers (e.g. WebSocketProvider) that the token has changed
    // in-tab so they can reconnect with the fresh credential.
    try {
      window.dispatchEvent(new CustomEvent("auth:token-changed"));
    } catch {
      // Older browsers — best-effort.
    }

    return true;
  } catch {
    console.error(
      "Token refresh failed, clearing session and redirecting to login",
    );
    clearSessionAndRedirect();
    return false;
  }
}

// Helper to proactively refresh token if needed
export async function ensureValidToken(): Promise<void> {
  const token = localStorage.getItem("access_token");
  const refreshToken = localStorage.getItem("refresh_token");

  if (!token || !refreshToken) {
    return; // No tokens to refresh
  }

  // Check if token is expiring soon (within 5 minutes)
  if (isTokenExpiringSoon(token, 300)) {
    await attemptTokenRefresh();
  }
}

// Set up automatic token refresh check
let tokenCheckInterval: ReturnType<typeof setInterval> | null = null;

export function startTokenRefreshMonitor(): void {
  if (tokenCheckInterval) {
    return; // Already running
  }

  // Check token every 60 seconds
  tokenCheckInterval = setInterval(async () => {
    const token = localStorage.getItem("access_token");
    if (token && !isTokenExpired(token)) {
      await ensureValidToken();
    }
  }, 60000);

  // Also check immediately
  ensureValidToken();
}

export function stopTokenRefreshMonitor(): void {
  if (tokenCheckInterval) {
    clearInterval(tokenCheckInterval);
    tokenCheckInterval = null;
  }
}

// Configure axios defaults to apply to all instances
export function configureAxiosDefaults(): void {
  // Set default base URL
  if (API_BASE_URL) {
    axios.defaults.baseURL = API_BASE_URL;
  }

  // Set default headers
  axios.defaults.headers.common["Content-Type"] = "application/json";

  // Request interceptor — attach JWT to outgoing requests
  axios.interceptors.request.use(
    (config) => {
      const token = localStorage.getItem("access_token");
      if (token && config.headers) {
        config.headers.Authorization = `Bearer ${token}`;
      }
      return config;
    },
    (error) => {
      return Promise.reject(error);
    },
  );

  // Response interceptor — handle 401 with a single refresh attempt
  axios.interceptors.response.use(
    (response) => response,
    async (error) => {
      const originalRequest = error.config as InternalAxiosRequestConfig & {
        _retry?: boolean;
      };

      // Handle 401 Unauthorized — token expired or invalid
      if (error.response?.status === 401 && !originalRequest._retry) {
        originalRequest._retry = true;

        const refreshed = await attemptTokenRefresh();
        if (refreshed) {
          // Retry original request with new token
          const newToken = localStorage.getItem("access_token");
          if (originalRequest.headers && newToken) {
            originalRequest.headers.Authorization = `Bearer ${newToken}`;
          }
          return axios(originalRequest);
        }

        // attemptTokenRefresh already cleared session and redirected
        return Promise.reject(error);
      }

      // Handle 403 Forbidden — valid token but insufficient permissions
      if (error.response?.status === 403) {
        error.isAuthorizationError = true;

        console.warn(
          "Access forbidden - insufficient permissions for this resource",
        );
        return Promise.reject(error);
      }

      return Promise.reject(error);
    },
  );
}

// Initialize the API wrapper
export function initializeApiWrapper(): void {
  configureAxiosDefaults();
}
