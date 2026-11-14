import { refreshSession } from "./session-refresh";
import axios, {
  type AxiosError,
  type AxiosInstance,
  type InternalAxiosRequestConfig,
} from "axios";

const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || "";

// Clear auth state and redirect to the login page.
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

// Create axios instance
export const apiClient: AxiosInstance = axios.create({
  baseURL: API_BASE_URL,
  timeout: 30000,
  headers: {
    "Content-Type": "application/json",
  },
});

// Request interceptor - add JWT token
apiClient.interceptors.request.use(
  (config: InternalAxiosRequestConfig) => {
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

// Response interceptor - handle token refresh and authentication errors
apiClient.interceptors.response.use(
  (response) => response,
  async (error: AxiosError) => {
    const originalRequest = error.config as InternalAxiosRequestConfig & {
      _retry?: boolean;
    };

    // Handle 401 Unauthorized — token expired or invalid
    if (error.response?.status === 401 && !originalRequest._retry) {
      originalRequest._retry = true;

      try {
        const access_token = await refreshSession();

        // Retry original request with new token
        if (originalRequest.headers) {
          originalRequest.headers.Authorization = `Bearer ${access_token}`;
        }
        return apiClient(originalRequest);
      } catch (refreshError) {
        // Refresh failed - clear tokens and redirect to login
        console.error(
          "Token refresh failed, clearing session and redirecting to login",
        );
        clearSessionAndRedirect();
        return Promise.reject(refreshError);
      }
    }

    // Handle 403 Forbidden - valid token but insufficient permissions
    if (error.response?.status === 403) {
      const enhancedError = error as AxiosError & {
        isAuthorizationError?: boolean;
      };
      enhancedError.isAuthorizationError = true;

      console.warn(
        "Access forbidden - insufficient permissions for this resource",
      );
      return Promise.reject(enhancedError);
    }

    return Promise.reject(error);
  },
);
