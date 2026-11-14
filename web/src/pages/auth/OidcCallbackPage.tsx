import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useAuth } from "@/contexts/AuthContext";

function parseHashParams(hash: string): URLSearchParams {
  const fragment = hash.startsWith("#") ? hash.slice(1) : hash;
  return new URLSearchParams(fragment);
}

export default function OidcCallbackPage() {
  const navigate = useNavigate();
  const { completeLogin } = useAuth();
  const [error, setError] = useState<string | null>(null);

  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    const finalizeLogin = async () => {
      const params = parseHashParams(window.location.hash);
      const code = params.get("code");
      window.history.replaceState(null, "", window.location.pathname);

      if (!code) {
        setError("Missing login exchange code.");
        return;
      }

      try {
        const response = await fetch("/auth/exchange", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ code }),
        });
        if (!response.ok)
          throw new Error("Login exchange expired or already used.");
        const { data } = await response.json();
        const redirectTo = data.redirect_to;
        await completeLogin({
          accessToken: data.tokens.access_token,
          refreshToken: data.tokens.refresh_token,
        });
        sessionStorage.removeItem("redirect_after_login");
        navigate(redirectTo, { replace: true });
      } catch (err) {
        const message =
          err instanceof Error ? err.message : "Failed to complete login.";
        setError(message);
      }
    };

    void finalizeLogin();
  }, [completeLogin, navigate]);

  return (
    <div className="min-h-screen flex items-center justify-center bg-gray-50 px-4">
      <div className="w-full max-w-md rounded-2xl border border-gray-200 bg-white p-8 shadow-sm">
        <h1 className="text-2xl font-semibold text-gray-900">
          Completing sign-in
        </h1>
        <p className="mt-3 text-sm text-gray-600">
          Attune is finalizing your authenticated session.
        </p>
        {error ? (
          <div className="mt-6 rounded-lg bg-red-50 p-4 text-sm text-red-700">
            {error}
          </div>
        ) : (
          <div className="mt-6 flex items-center gap-3 text-sm text-gray-600">
            <div className="h-4 w-4 animate-spin rounded-full border-2 border-gray-300 border-t-gray-900" />
            Redirecting...
          </div>
        )}
      </div>
    </div>
  );
}
