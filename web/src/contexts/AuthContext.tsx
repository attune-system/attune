/* eslint-disable react-refresh/only-export-components -- exporting useAuth hook alongside AuthProvider is standard React pattern */
import {
  createContext,
  useContext,
  useState,
  useEffect,
  ReactNode,
} from "react";
import { AuthService, ApiError } from "@/api";
import type { CurrentUserResponse } from "@/api";
import {
  startTokenRefreshMonitor,
  stopTokenRefreshMonitor,
} from "@/lib/api-wrapper";

interface AuthContextType {
  user: CurrentUserResponse | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  login: (redirectTo?: string) => void;
  logout: () => void;
  refreshUser: () => Promise<void>;
  getToken: () => string | null;
  completeLogin: (params: {
    accessToken: string;
    refreshToken: string;
  }) => Promise<void>;
}

const AuthContext = createContext<AuthContextType | undefined>(undefined);

interface AuthProviderProps {
  children: ReactNode;
}

export function AuthProvider({ children }: AuthProviderProps) {
  const [user, setUser] = useState<CurrentUserResponse | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  async function loadUser() {
    const token = localStorage.getItem("access_token");
    if (!token) {
      setIsLoading(false);
      return;
    }

    try {
      const response = await AuthService.getCurrentUser();
      setUser(response.data);
    } catch (error) {
      console.error("Failed to load user:", error);
      if (error instanceof ApiError) {
        console.error(`API Error ${error.status}: ${error.message}`);
      }
      localStorage.removeItem("access_token");
      localStorage.removeItem("refresh_token");
      setUser(null);
    } finally {
      setIsLoading(false);
    }
  }

  /* eslint-disable react-hooks/set-state-in-effect -- initializes authentication state from the persisted session */
  useEffect(() => {
    void loadUser();
  }, []);
  /* eslint-enable react-hooks/set-state-in-effect */

  // Start/stop token refresh monitoring based on auth state
  useEffect(() => {
    if (user) {
      startTokenRefreshMonitor();
    } else {
      stopTokenRefreshMonitor();
    }

    return () => {
      stopTokenRefreshMonitor();
    };
  }, [user]);

  const login = (redirectTo?: string) => {
    const redirectParam = redirectTo
      ? `?redirect_to=${encodeURIComponent(redirectTo)}`
      : "";
    window.location.href = `/auth/oidc/login${redirectParam}`;
  };

  const logout = () => {
    localStorage.removeItem("access_token");
    localStorage.removeItem("refresh_token");
    stopTokenRefreshMonitor();
    setUser(null);
    window.location.href = "/auth/logout";
  };

  const refreshUser = async () => {
    await loadUser();
  };

  const getToken = () => {
    return localStorage.getItem("access_token");
  };

  const completeLogin = async ({
    accessToken,
    refreshToken,
  }: {
    accessToken: string;
    refreshToken: string;
  }) => {
    localStorage.setItem("access_token", accessToken);
    localStorage.setItem("refresh_token", refreshToken);
    try {
      window.dispatchEvent(new CustomEvent("auth:token-changed"));
    } catch {
      // Older browsers — best-effort.
    }
    await loadUser();
  };

  const value: AuthContextType = {
    user,
    isAuthenticated: !!user,
    isLoading,
    login,
    logout,
    refreshUser,
    getToken,
    completeLogin,
  };

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const context = useContext(AuthContext);
  if (context === undefined) {
    throw new Error("useAuth must be used within an AuthProvider");
  }
  return context;
}
