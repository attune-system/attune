import type { ReactNode } from "react";
import { Blob as NodeBlob } from "node:buffer";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ApiError,
  CacheGenerationState,
  CacheRefreshConcurrency,
  OwnerType,
  type CacheGenerationResponse,
  type CacheNamespaceResponse,
  type CreateCacheGenerationRequest,
} from "@/api";
import CacheRefreshTab from "./CacheRefreshTab";

const service = vi.hoisted(() => ({
  createGeneration: vi.fn(),
  uploadChunk: vi.fn(),
  sealGeneration: vi.fn(),
  promoteGeneration: vi.fn(),
  abandonGeneration: vi.fn(),
  showNamespace: vi.fn(),
  showGeneration: vi.fn(),
}));
const auth = vi.hoisted(() => ({ canReadExecutions: false }));

vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return { ...actual, CachesService: service };
});
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ user: null }) }));
vi.mock("@/lib/permissions", () => ({
  hasPermission: (_user: unknown, resource: string, action: string) =>
    resource === "executions" && action === "read" && auth.canReadExecutions,
}));

const namespace: CacheNamespaceResponse = {
  id: 1,
  namespace: "users",
  owner_type: OwnerType.SYSTEM,
  owner: "system",
  owner_ref: null,
  managed: false,
  managing_pack_ref: null,
  definition_ref: null,
  active_generation: null,
  cache_not_populated: true,
  refresh_concurrency: CacheRefreshConcurrency.REUSE,
  freshness_target_seconds: 3600,
  max_records_per_generation: 1000,
  max_generation_bytes: 1024,
  max_retained_bytes: 4096,
  max_retained_generations: 2,
  max_staging_generations: 2,
  tombstoned: false,
  retired_at: null,
  created: "2026-10-07T00:00:00Z",
  updated: "2026-10-07T00:00:00Z",
  stale: false,
  record_count: null,
  size_bytes: null,
  source_revision: null,
  last_refreshed_at: null,
};

function generation(
  clientRefreshId: string,
  status = CacheGenerationState.STAGING,
  executionId: number | null = 9876543210,
): CacheGenerationResponse {
  return {
    generation_id: 42,
    namespace_id: 1,
    status,
    client_refresh_id: clientRefreshId,
    created_by: null,
    created_by_execution: executionId,
    expected_active_generation_id: null,
    expected_chunk_count: 1,
    expected_record_count: 1,
    expected_size_bytes: null,
    source_revision: "original",
    record_count: 1,
    size_bytes: 12,
    checksum: null,
    checksum_algorithm: null,
    created: "2026-10-07T00:00:00Z",
    sealed: null,
    activated: null,
    retired: null,
    readable_until: null,
    failed: null,
    failure_reason: null,
  };
}

function createWrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <MemoryRouter>
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      </MemoryRouter>
    );
  };
}

beforeEach(() => {
  Object.values(service).forEach((mock) => mock.mockReset());
  auth.canReadExecutions = false;
  service.showNamespace.mockResolvedValue({ data: namespace });
  service.showGeneration.mockResolvedValue({
    data: generation("other-producer"),
  });
});

async function prepareAndBegin() {
  const user = userEvent.setup();
  const { container } = render(
    <CacheRefreshTab
      owner={{ ownerType: OwnerType.SYSTEM }}
      namespaceName="users"
      namespace={namespace}
    />,
    { wrapper: createWrapper() },
  );
  // jsdom's File lacks arrayBuffer. Use the native Blob for the file's bounded reads.
  const file = new File(
    ['{"external_id":"one","value":{}}\n'],
    "users.ndjson",
    { lastModified: 1 },
  );
  Object.defineProperty(file, "slice", {
    value: (start: number, end: number) =>
      new NodeBlob(['{"external_id":"one","value":{}}\n']).slice(start, end),
  });
  const input = container.querySelector('input[type="file"]');
  if (!input) throw new Error("File input missing");
  fireEvent.change(input, { target: { files: [file] } });
  await user.click(
    screen.getByRole("button", { name: "Prepare (count records)" }),
  );
  await user.click(
    await screen.findByRole("button", { name: /2\. Begin refresh/ }),
  );
  return user;
}

describe("CacheRefreshTab", () => {
  it.each([CacheGenerationState.STAGING, CacheGenerationState.READY])(
    "never writes to another producer's reused %s generation",
    async (status) => {
      service.createGeneration.mockResolvedValue({
        data: generation("other-producer", status),
      });
      await prepareAndBegin();
      expect(
        await screen.findByText(/Another refresh was reused/),
      ).toBeInTheDocument();
      expect(screen.getByText("other-producer")).toBeInTheDocument();
      expect(screen.getByText("Execution #9876543210")).toBeInTheDocument();
      expect(
        screen.queryByRole("link", { name: "Execution #9876543210" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", {
          name: /Upload chunks|Seal generation|Promote \(|Abandon this refresh/,
        }),
      ).not.toBeInTheDocument();
      expect(service.uploadChunk).not.toHaveBeenCalled();
      expect(service.sealGeneration).not.toHaveBeenCalled();
      expect(service.promoteGeneration).not.toHaveBeenCalled();
      expect(service.abandonGeneration).not.toHaveBeenCalled();
      expect(service.createGeneration).toHaveBeenCalledTimes(1);
      expect(
        service.createGeneration.mock.calls[0]?.[0].requestBody,
      ).not.toHaveProperty("created_by_execution");
    },
  );

  it("links the historical producer only with execution-read permission and checks status manually", async () => {
    auth.canReadExecutions = true;
    service.createGeneration.mockResolvedValue({
      data: generation("other-producer"),
    });
    const user = await prepareAndBegin();
    expect(
      await screen.findByRole("link", { name: "Execution #9876543210" }),
    ).toHaveAttribute("href", "/executions/9876543210");
    await waitFor(() =>
      expect(service.showGeneration).toHaveBeenCalledTimes(1),
    );
    service.showGeneration.mockResolvedValue({
      data: generation("other-producer", CacheGenerationState.ACTIVE),
    });
    await user.click(
      screen.getByRole("button", { name: "Check generation status" }),
    );
    expect(
      await screen.findByText("Generation status: active"),
    ).toBeInTheDocument();
    expect(service.showGeneration).toHaveBeenCalledTimes(2);
    expect(service.uploadChunk).not.toHaveBeenCalled();
  });

  it("shows structured refresh conflict metadata without writing", async () => {
    service.createGeneration.mockRejectedValue(
      new ApiError(
        { method: "POST", url: "/cache" },
        {
          url: "/cache",
          ok: false,
          status: 409,
          statusText: "Conflict",
          body: {
            code: "cache_refresh_in_progress",
            details: { generation_id: 42, created_by_execution: null },
          },
        },
        "Refresh in progress",
      ),
    );
    await prepareAndBegin();
    expect(
      await screen.findByText(/A refresh is already in progress/),
    ).toBeInTheDocument();
    expect(screen.getByText("None recorded")).toBeInTheDocument();
    expect(service.uploadChunk).not.toHaveBeenCalled();
  });

  it("allows matching client-refresh-ID replay through real upload, seal, and promote mutations", async () => {
    service.createGeneration.mockImplementation(
      ({ requestBody }: { requestBody: CreateCacheGenerationRequest }) =>
        Promise.resolve({
          data: generation(
            requestBody.client_refresh_id,
            CacheGenerationState.STAGING,
            null,
          ),
        }),
    );
    service.uploadChunk.mockResolvedValue({
      data: generation("matching", CacheGenerationState.STAGING, null),
    });
    service.sealGeneration.mockResolvedValue({
      data: generation("matching", CacheGenerationState.READY, null),
    });
    service.promoteGeneration.mockResolvedValue({
      data: generation("matching", CacheGenerationState.ACTIVE, null),
    });
    const user = await prepareAndBegin();
    await user.click(
      await screen.findByRole("button", { name: "3. Upload chunks" }),
    );
    await waitFor(() => expect(service.uploadChunk).toHaveBeenCalledTimes(1));
    await user.click(
      screen.getByRole("button", { name: "4. Seal generation" }),
    );
    await user.click(
      await screen.findByRole("button", {
        name: "Promote (expects active = none)",
      }),
    );
    expect(
      await screen.findByText(/Promoted\. This generation/),
    ).toBeInTheDocument();
    expect(service.sealGeneration).toHaveBeenCalledTimes(1);
    expect(service.promoteGeneration).toHaveBeenCalledTimes(1);
  });
});
