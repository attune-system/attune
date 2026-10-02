import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PacksService } from "@/api";
import PackInstallPage from "./PackInstallPage";

vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ user: {} }) }));
vi.mock("@/lib/permissions", () => ({ hasPermission: () => true }));
vi.mock("@/hooks/usePackTests", () => ({
  useInstallPack: () => ({ mutateAsync: vi.fn(), isPending: false }),
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
  created: "2026-09-30T00:00:00Z",
  updated: "2026-09-30T00:00:00Z",
};

beforeEach(() => {
  vi.spyOn(PacksService, "listPackIndices").mockResolvedValue({
    data: [index],
  });
  vi.spyOn(PacksService, "browseIndexedPacks").mockResolvedValue({ data: [] });
});
afterEach(() => vi.restoreAllMocks());

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
  await userEvent.click(screen.getByRole("button", { name: "Save index" }));
  await waitFor(() =>
    expect(update).toHaveBeenCalledWith({
      id: 42,
      requestBody: {
        name: null,
        url: index.url,
        enabled: true,
        headers: {},
      },
    }),
  );
});
