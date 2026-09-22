import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ExternalIdentityMappingsService,
  type ExternalIdentityMappingResponse,
} from "@/api";
import { ExternalIdentityMappingsSection } from "@/components/access-control/ExternalIdentityMappingsSection";

vi.mock("@/api", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/api")>();
  return {
    ...original,
    ExternalIdentityMappingsService: {
      listExternalIdentityMappings: vi.fn(),
      createExternalIdentityMapping: vi.fn(),
      updateExternalIdentityMapping: vi.fn(),
      deleteExternalIdentityMapping: vi.fn(),
    },
  };
});

const mapping: ExternalIdentityMappingResponse = {
  id: 7,
  integration_identity: 2,
  mapped_identity: 42,
  provider: "slack",
  tenant: "workspace-1",
  subject_kind: "user",
  external_subject: "user-9",
  created_by: 1,
  created: "2026-09-22T18:00:00Z",
  updated: "2026-09-22T18:00:00Z",
};

function renderSection() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <ExternalIdentityMappingsSection integrationIdentity={2} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("ExternalIdentityMappingsSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(
      ExternalIdentityMappingsService.listExternalIdentityMappings,
    ).mockResolvedValue({
      items: [mapping],
      pagination: {
        page: 1,
        page_size: 20,
        has_next: false,
        has_previous: false,
        total_items: 1,
        total_pages: 1,
      },
    });
  });

  it("shows the exact provider tuple and mapped Attune identity", async () => {
    renderSection();

    expect(await screen.findByText("workspace-1")).toBeInTheDocument();
    expect(screen.getByText("user-9")).toBeInTheDocument();
    expect(screen.getByText("slack")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Attune identity 42" }),
    ).toHaveAttribute("href", "/access-control/identities/42");
  });

  it("creates a mapping for this integration identity", async () => {
    vi.mocked(
      ExternalIdentityMappingsService.createExternalIdentityMapping,
    ).mockResolvedValue({ data: mapping });
    const user = userEvent.setup();
    renderSection();

    await screen.findByText("workspace-1");
    await user.click(screen.getByRole("button", { name: "Add mapping" }));
    await user.type(screen.getByLabelText("Provider"), "slack");
    await user.type(screen.getByLabelText("Tenant"), "workspace-1");
    await user.type(screen.getByLabelText("External subject"), "user-9");
    await user.type(screen.getByLabelText("Mapped Attune identity ID"), "42");
    await user.click(screen.getByRole("button", { name: "Create mapping" }));

    await waitFor(() =>
      expect(
        ExternalIdentityMappingsService.createExternalIdentityMapping,
      ).toHaveBeenCalledWith({
        integrationIdentity: 2,
        requestBody: {
          mapped_identity: 42,
          provider: "slack",
          tenant: "workspace-1",
          subject_kind: "user",
          external_subject: "user-9",
        },
      }),
    );
  });

  it("updates an existing mapping", async () => {
    vi.mocked(
      ExternalIdentityMappingsService.updateExternalIdentityMapping,
    ).mockResolvedValue({ data: { ...mapping, mapped_identity: 43 } });
    const user = userEvent.setup();
    renderSection();

    await screen.findByText("workspace-1");
    await user.click(
      screen.getByRole("button", { name: "Edit slack mapping" }),
    );
    const mappedIdentity = screen.getByLabelText("Mapped Attune identity ID");
    await user.clear(mappedIdentity);
    await user.type(mappedIdentity, "43");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() =>
      expect(
        ExternalIdentityMappingsService.updateExternalIdentityMapping,
      ).toHaveBeenCalledWith({
        integrationIdentity: 2,
        mappingId: 7,
        requestBody: {
          mapped_identity: 43,
          provider: "slack",
          tenant: "workspace-1",
          subject_kind: "user",
          external_subject: "user-9",
        },
      }),
    );
  });
});
