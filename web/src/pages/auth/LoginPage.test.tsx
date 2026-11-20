import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { expect, it, vi } from "vitest";
import { InstallationSettingsProvider } from "@/contexts/InstallationSettingsContext";
import LoginPage from "./LoginPage";

const { getSettings } = vi.hoisted(() => ({ getSettings: vi.fn() }));

vi.mock("axios", async (importOriginal) => {
  const axios = await importOriginal<typeof import("axios")>();
  const create = axios.default.create;
  return {
    ...axios,
    default: {
      ...axios.default,
      create: (...args: Parameters<typeof create>) =>
        Object.assign(create(...args), { get: getSettings }),
    },
  };
});

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ login: vi.fn(), completeLogin: vi.fn() }),
}));

const authSettings = {
  authentication_enabled: true,
  local_password_enabled: true,
  local_password_visible_by_default: true,
  oidc_enabled: true,
  oidc_visible_by_default: true,
  ldap_enabled: false,
  ldap_visible_by_default: false,
  self_registration_enabled: false,
};

function renderLogin() {
  return render(
    <InstallationSettingsProvider>
      <MemoryRouter initialEntries={["/login"]}>
        <LoginPage />
      </MemoryRouter>
    </InstallationSettingsProvider>,
  );
}

it("shows the configured plain-text disclosure and branding before sign-in", async () => {
  const favicon = document.createElement("link");
  favicon.rel = "icon";
  favicon.type = "image/svg+xml";
  favicon.href = "/attune-favicon.svg";
  document.head.append(favicon);
  const disclosure =
    "Authorized use only.\n\n<strong>Read the policy.</strong>";
  getSettings.mockResolvedValueOnce({
    data: {
      data: {
        ...authSettings,
        login_disclosure: disclosure,
        branding: {
          logo_url: "/branding/logo.svg",
          favicon_url: "/branding/favicon.png",
        },
      },
    },
  });

  const { unmount } = renderLogin();
  try {
    const banner = await screen.findByRole("region", {
      name: "Login disclosure",
    });
    expect(
      within(banner).getByText(disclosure, { normalizer: (text) => text })
        .textContent,
    ).toBe(disclosure);
    expect(banner.querySelector("strong")).toBeNull();
    for (const name of ["Sign in", "Continue with SSO"]) {
      const control = screen.getByRole("button", { name });
      expect(
        banner.compareDocumentPosition(control) &
          Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
    }
    expect(screen.getByRole("img", { name: "Attune" })).toHaveAttribute(
      "src",
      "/branding/logo.svg",
    );
    expect(favicon).toHaveAttribute("href", "/branding/favicon.png");
    expect(favicon).not.toHaveAttribute("type");
  } finally {
    unmount();
    favicon.remove();
  }
});

it("keeps the default sign-in heading and controls with optional settings unset", async () => {
  getSettings.mockResolvedValueOnce({
    data: { data: { ...authSettings, branding: {} } },
  });

  renderLogin();

  expect(await screen.findByRole("button", { name: "Sign in" })).toBeVisible();
  expect(screen.getByRole("heading", { name: "Attune" })).toHaveTextContent(
    "Attune",
  );
  expect(screen.queryByRole("region", { name: "Login disclosure" })).toBeNull();
});
