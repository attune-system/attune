import { StrictMode } from "react";
import { render, waitFor, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
const { navigate, completeLogin } = vi.hoisted(() => ({
  navigate: vi.fn(),
  completeLogin: vi.fn(),
}));
vi.mock("react-router-dom", () => ({ useNavigate: () => navigate }));
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ completeLogin }),
}));
import OidcCallbackPage from "./OidcCallbackPage";
beforeEach(() => {
  vi.clearAllMocks();
  completeLogin.mockResolvedValue(undefined);
  vi.stubGlobal("sessionStorage", { removeItem: vi.fn() });
  window.history.replaceState(null, "", "/login/callback#code=one-use");
});
it("redeems once in StrictMode, removes code and completes login with server redirect", async () => {
  const fetch = vi.fn().mockResolvedValue({
    ok: true,
    json: async () => ({
      data: {
        tokens: { access_token: "access", refresh_token: "refresh" },
        redirect_to: "/executions",
      },
    }),
  });
  vi.stubGlobal("fetch", fetch);
  render(
    <StrictMode>
      <OidcCallbackPage />
    </StrictMode>,
  );
  await waitFor(() =>
    expect(navigate).toHaveBeenCalledWith("/executions", { replace: true }),
  );
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(fetch).toHaveBeenCalledWith(
    "/auth/exchange",
    expect.objectContaining({ body: JSON.stringify({ code: "one-use" }) }),
  );
  expect(window.location.hash).toBe("");
  expect(completeLogin).toHaveBeenCalledWith({
    accessToken: "access",
    refreshToken: "refresh",
  });
});
it("does not complete login when the one-use exchange is rejected", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValue({ ok: false }));
  render(<OidcCallbackPage />);
  await screen.findByText("Login exchange expired or already used.");
  expect(completeLogin).not.toHaveBeenCalled();
  expect(navigate).not.toHaveBeenCalled();
});
