import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PacksService } from "@/api";
import PackInstallPage from "./PackInstallPage";

const installPackMock = vi.hoisted(() => vi.fn());
const installStatusMock = vi.hoisted(() => vi.fn());

vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ user: {} }) }));
vi.mock("@/lib/permissions", () => ({ hasPermission: () => true }));
vi.mock("@/hooks/usePackTests", () => ({
  useInstallPack: () => ({ mutateAsync: installPackMock, isPending: false }),
  usePackInstallStatus: installStatusMock,
}));
vi.mock("@/components/packs/PlatformCatalogStatus", () => ({
  default: () => null,
}));

const index = {
  id: 42,
  name: "Private index",
  url: "https://registry.example.com/index.json",
  position: 0,
  enabled: true,
  headers: { Authorization: "[REDACTED]" },
  source_headers: {
    "https://artifacts.example.com": { Authorization: "[REDACTED]" },
  },
  created: "2026-09-30T00:00:00Z",
  updated: "2026-09-30T00:00:00Z",
};

beforeEach(() => {
  installPackMock.mockReset();
  installStatusMock
    .mockReset()
    .mockReturnValue({ data: undefined, error: null });
  vi.spyOn(PacksService, "listPackIndices").mockResolvedValue({
    data: [index],
  });
  vi.spyOn(PacksService, "browseIndexedPacks").mockResolvedValue({ data: [] });
});
afterEach(() => vi.restoreAllMocks());

async function submitInstall() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = render(<PackInstallPage />, {
    wrapper: ({ children }) => (
      <QueryClientProvider client={client}>
        <MemoryRouter>{children}</MemoryRouter>
      </QueryClientProvider>
    ),
  });
  await userEvent.type(
    screen.getByLabelText(/Git Repository URL/),
    "https://example.invalid/demo.git",
  );
  await userEvent.click(screen.getByRole("button", { name: "Install Pack" }));
  return view;
}

it("keeps failed metadata and authored field paths visible without success", async () => {
  installPackMock.mockRejectedValueOnce({
    body: {
      error: "Invalid metadata",
      details: {
        stage: "metadata_validation",
        outcome: "active_release_unchanged",
        diagnostics: [
          {
            path: "actions/run.yaml",
            metadata_path: "/timeout_seconds",
            message: "Timeout must be positive",
            hint: "Use seconds",
          },
        ],
      },
    },
  });
  await submitInstall();
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("actions/run.yaml at /timeout_seconds");
  expect(alert).toHaveTextContent("Use seconds");
  expect(screen.queryByText(/installed successfully/)).not.toBeInTheDocument();
});

it("does not claim success or permit another submission while activation is pending", async () => {
  installPackMock.mockResolvedValueOnce({
    data: {
      pack: { ref: "demo", label: "Demo", version: "1.0.0" },
      install_id: 42,
      install_status: "activating",
      tests_skipped: false,
    },
  });
  await submitInstall();
  expect(await screen.findByRole("status")).toHaveTextContent(
    "still activating",
  );
  expect(screen.getByRole("button", { name: "Install Pack" })).toBeDisabled();
  expect(screen.queryByText(/installed successfully/)).not.toBeInTheDocument();
});

function pendingInstall() {
  installPackMock.mockResolvedValueOnce({
    data: {
      pack: { ref: "demo", label: "Demo", version: "1.0.0" },
      install_id: 42,
      install_status: "activating",
      tests_skipped: false,
    },
  });
}

it("shows polled deployment failures and permits a new attempt", async () => {
  pendingInstall();
  const view = await submitInstall();
  const failedStatus = {
    data: {
      data: { status: "failed", error_message: "Candidate tests failed" },
    },
    error: null,
  };
  installStatusMock.mockImplementation(
    (_ref: string, id: number | undefined) =>
      id === 42 ? failedStatus : { data: undefined, error: null },
  );
  view.rerender(<PackInstallPage />);
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Candidate tests failed",
  );
  expect(screen.queryByRole("status")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Install Pack" })).toBeEnabled();
  expect(screen.queryByText(/deployment completed/)).not.toBeInTheDocument();
  installPackMock.mockResolvedValueOnce({
    data: {
      pack: { ref: "demo", label: "Demo", version: "1.0.0" },
      install_id: 43,
      install_status: "running",
      tests_skipped: false,
    },
  });
  await userEvent.click(screen.getByRole("button", { name: "Install Pack" }));
  expect(await screen.findByRole("status")).toHaveTextContent("still running");
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  expect(installStatusMock).toHaveBeenLastCalledWith("demo", 43, true);
  expect(screen.getByRole("button", { name: "Install Pack" })).toBeDisabled();
});

it("keeps success and warning details visible when polling completes", async () => {
  pendingInstall();
  const view = await submitInstall();
  const completedStatus = {
    data: {
      data: {
        status: "succeeded",
        diagnostic_details: {
          diagnostics: [{ severity: "warning", message: "Tests were skipped" }],
        },
      },
    },
    error: null,
  };
  installStatusMock.mockReturnValue(completedStatus);
  view.rerender(<PackInstallPage />);
  expect(
    await screen.findByText("Pack 'demo' deployment completed."),
  ).toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent("Tests were skipped");
  expect(screen.queryByText(/still activating/)).not.toBeInTheDocument();
  view.rerender(<PackInstallPage />);
  expect(
    screen.getByText("Pack 'demo' deployment completed."),
  ).toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent("Tests were skipped");
  installStatusMock.mockReturnValue({
    ...completedStatus,
    error: new Error("Status unavailable after completion"),
  });
  view.rerender(<PackInstallPage />);
  expect(
    screen.getByText("Pack 'demo' deployment completed."),
  ).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("clears transient status errors when polling recovers", async () => {
  pendingInstall();
  const view = await submitInstall();
  installStatusMock.mockReturnValue({
    data: undefined,
    error: new Error("Status unavailable"),
  });
  view.rerender(<PackInstallPage />);
  expect(await screen.findByRole("alert")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Install Pack" })).toBeDisabled();
  installStatusMock.mockReturnValue({
    data: { data: { status: "succeeded" } },
    error: null,
  });
  view.rerender(<PackInstallPage />);
  expect(
    await screen.findByText("Pack 'demo' deployment completed."),
  ).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("leaves deployment warnings visible after a successful response", async () => {
  installPackMock.mockResolvedValueOnce({
    data: {
      pack: { ref: "demo", label: "Demo", version: "1.0.0" },
      install_status: "succeeded",
      tests_skipped: true,
      diagnostics: [
        {
          severity: "warning",
          code: "testing.skipped",
          path: "pack.yaml",
          metadata_path: "/testing",
          message: "Tests were skipped",
        },
      ],
    },
  });
  await submitInstall();
  expect(await screen.findByRole("status")).toHaveTextContent(
    "pack.yaml at /testing",
  );
  expect(
    screen.getByText(/Deployment details remain here/),
  ).toBeInTheDocument();
});

async function openIndices() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <PackInstallPage />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  await userEvent.click(
    screen.getByRole("button", { name: "Manage Pack Indices" }),
  );
  await screen.findByText("Private index");
}

it("allows request headers to be configured when adding an index", async () => {
  await openIndices();
  expect(screen.getByLabelText("HTTP headers")).toBeInTheDocument();
});

it("offers a per-index metadata refresh action", async () => {
  await openIndices();
  expect(
    screen.getByRole("button", { name: "Refresh Private index" }),
  ).toBeInTheDocument();
});

it("sends the configured source, enabled state, and authentication headers on create", async () => {
  const create = vi
    .spyOn(PacksService, "createPackIndex")
    .mockResolvedValue({ data: index });
  await openIndices();
  fireEvent.change(screen.getByLabelText("Index name"), {
    target: { value: "New private index" },
  });
  fireEvent.change(screen.getByLabelText("Index URL"), {
    target: { value: "https://registry.example.com/new.json" },
  });
  fireEvent.change(screen.getByLabelText("HTTP headers"), {
    target: { value: '{"Authorization":"Bearer test-value"}' },
  });
  fireEvent.change(screen.getByLabelText("Source headers by origin"), {
    target: {
      value: '{"https://git.example.com":{"Authorization":"Basic git-token"}}',
    },
  });
  await userEvent.click(
    screen.getByRole("checkbox", {
      name: "Enabled for pack discovery and installation",
    }),
  );
  await userEvent.click(screen.getByRole("button", { name: "Add Index" }));
  await waitFor(() =>
    expect(create).toHaveBeenCalledWith({
      requestBody: {
        name: "New private index",
        url: "https://registry.example.com/new.json",
        enabled: false,
        headers: { Authorization: "Bearer test-value" },
        source_headers: {
          "https://git.example.com": { Authorization: "Basic git-token" },
        },
      },
    }),
  );
});

it("allows editing without replacing stored redacted credentials", async () => {
  const update = vi
    .spyOn(PacksService, "updatePackIndex")
    .mockResolvedValue({ data: index });
  await openIndices();
  await userEvent.click(
    screen.getByRole("button", { name: "Edit Private index" }),
  );
  expect(screen.getByLabelText("Index name")).toHaveFocus();
  expect(screen.getByLabelText("HTTP headers")).toHaveValue(
    JSON.stringify(index.headers, null, 2),
  );
  expect(screen.getByLabelText("Source headers by origin")).toHaveValue(
    JSON.stringify(index.source_headers, null, 2),
  );
  fireEvent.change(screen.getByLabelText("Index URL"), {
    target: { value: "https://registry.example.com/main/index.json" },
  });
  await userEvent.click(screen.getByRole("button", { name: "Save index" }));
  await waitFor(() =>
    expect(update).toHaveBeenCalledWith({
      id: 42,
      requestBody: {
        name: "Private index",
        url: "https://registry.example.com/main/index.json",
        enabled: true,
        headers: { Authorization: "[REDACTED]" },
        source_headers: index.source_headers,
      },
    }),
  );
});

it("refreshes only the selected index and reports the result", async () => {
  await openIndices();
  await userEvent.click(
    screen.getByRole("button", { name: "Refresh Private index" }),
  );
  await waitFor(() =>
    expect(PacksService.browseIndexedPacks).toHaveBeenCalledWith({
      registryId: 42,
      includeDisabled: true,
    }),
  );
  expect(await screen.findByRole("status")).toHaveTextContent(
    "Fetched 0 pack entries from Private index",
  );
});

it("keeps the edit form's enabled state consistent after toggling its index", async () => {
  const update = vi.spyOn(PacksService, "updatePackIndex").mockResolvedValue({
    data: { ...index, enabled: false },
  });
  await openIndices();
  await userEvent.click(
    screen.getByRole("button", { name: "Edit Private index" }),
  );
  await userEvent.click(screen.getByRole("button", { name: "Disable" }));
  expect(await screen.findByRole("status")).toHaveTextContent(
    "Index disabled.",
  );
  expect(
    screen.getByRole("checkbox", {
      name: "Enabled for pack discovery and installation",
    }),
  ).not.toBeChecked();
  fireEvent.change(screen.getByLabelText("Index name"), {
    target: { value: "Renamed index" },
  });
  await userEvent.click(screen.getByRole("button", { name: "Save index" }));
  await waitFor(() =>
    expect(update).toHaveBeenLastCalledWith({
      id: 42,
      requestBody: {
        name: "Renamed index",
        url: index.url,
        headers: index.headers,
        source_headers: index.source_headers,
        enabled: false,
      },
    }),
  );
});

it("shows metadata fetch failures inside the dialog without reporting success", async () => {
  await openIndices();
  vi.mocked(PacksService.browseIndexedPacks).mockRejectedValueOnce({
    body: { error: "Index authentication failed" },
  });
  await userEvent.click(
    screen.getByRole("button", { name: "Refresh Private index" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Index authentication failed",
  );
  expect(screen.queryByRole("status")).not.toBeInTheDocument();
});

it("rejects non-string header values before submitting", async () => {
  const create = vi.spyOn(PacksService, "createPackIndex");
  await openIndices();
  fireEvent.change(screen.getByLabelText("Index URL"), {
    target: { value: "https://registry.example.com/new.json" },
  });
  fireEvent.change(screen.getByLabelText("HTTP headers"), {
    target: { value: '{"X-Custom":42}' },
  });
  await userEvent.click(screen.getByRole("button", { name: "Add Index" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "HTTP header values must all be strings",
  );
  expect(create).not.toHaveBeenCalled();
});

it("rejects non-string source header values before submitting", async () => {
  const create = vi.spyOn(PacksService, "createPackIndex");
  await openIndices();
  fireEvent.change(screen.getByLabelText("Index URL"), {
    target: { value: "https://registry.example.com/new.json" },
  });
  fireEvent.change(screen.getByLabelText("Source headers by origin"), {
    target: { value: '{"https://git.example.com":{"Authorization":42}}' },
  });
  await userEvent.click(screen.getByRole("button", { name: "Add Index" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "HTTP header values must all be strings",
  );
  expect(create).not.toHaveBeenCalled();
});

it("allows clearing the name and headers when editing", async () => {
  const update = vi
    .spyOn(PacksService, "updatePackIndex")
    .mockResolvedValue({ data: index });
  await openIndices();
  await userEvent.click(
    screen.getByRole("button", { name: "Edit Private index" }),
  );
  fireEvent.change(screen.getByLabelText("Index name"), {
    target: { value: "" },
  });
  fireEvent.change(screen.getByLabelText("HTTP headers"), {
    target: { value: "{}" },
  });
  fireEvent.change(screen.getByLabelText("Source headers by origin"), {
    target: { value: "{}" },
  });
  await userEvent.click(screen.getByRole("button", { name: "Save index" }));
  await waitFor(() =>
    expect(update).toHaveBeenCalledWith({
      id: 42,
      requestBody: {
        name: null,
        url: index.url,
        enabled: true,
        headers: {},
        source_headers: {},
      },
    }),
  );
});
