import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { InfoService, OpenAPI } from "@/api";
import InfoPage from "./InfoPage";

afterEach(() => vi.restoreAllMocks());

function renderPage() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <InfoPage />
    </QueryClientProvider>,
  );
}

it("shows the responding server version rather than the web client's generated version", async () => {
  const serverVersion = "9.8.7";
  expect(serverVersion).not.toBe(OpenAPI.VERSION);
  vi.spyOn(InfoService, "getInfo").mockResolvedValue({
    data: { version: serverVersion, git_sha: "a".repeat(40) },
  });
  renderPage();
  expect(await screen.findByText(serverVersion)).toBeInTheDocument();
  expect(screen.getByText("a".repeat(40))).toBeInTheDocument();
});

it("shows a visible error when the server cannot report build information", async () => {
  vi.spyOn(InfoService, "getInfo").mockRejectedValue(
    new Error("Server unavailable"),
  );
  renderPage();
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Server unavailable",
  );
  expect(screen.queryByText(OpenAPI.VERSION)).not.toBeInTheDocument();
});
