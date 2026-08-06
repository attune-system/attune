import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { OwnerType, type CacheNamespaceResponse } from "@/api";
import CacheOverviewTab from "@/pages/caches/tabs/CacheOverviewTab";

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: {} }),
}));

vi.mock("@/lib/permissions", () => ({
  hasPermission: () => true,
}));

vi.mock("@/hooks/useCaches", () => ({
  useCacheGenerations: () => ({
    data: { data: { generations: [] } },
  }),
  useDeleteCacheNamespace: () => ({
    isPending: false,
    mutateAsync: vi.fn(),
  }),
}));

const managedNamespace: CacheNamespaceResponse = {
  id: 1,
  owner_type: OwnerType.PACK,
  owner: "1",
  owner_ref: "salesforce",
  namespace: "users",
  managed: true,
  definition_ref: "salesforce.users",
  managing_pack_ref: "salesforce",
  active_generation: 10,
  freshness_target_seconds: 0,
  max_records_per_generation: 1000,
  max_generation_bytes: 1024,
  max_retained_bytes: 4096,
  max_retained_generations: 2,
  max_staging_generations: 2,
  tombstoned: false,
  created: "2026-08-01T00:00:00Z",
  updated: "2026-08-01T00:00:00Z",
  cache_not_populated: false,
  stale: true,
  record_count: 10,
  size_bytes: 100,
  source_revision: null,
  last_refreshed_at: "2026-08-01T00:00:00Z",
};

describe("CacheOverviewTab", () => {
  it("shows managed provenance and pack guidance without delete controls", () => {
    render(
      <MemoryRouter>
        <CacheOverviewTab
          owner={{ ownerType: OwnerType.PACK, ownerRef: "salesforce" }}
          namespace={managedNamespace}
        />
      </MemoryRouter>,
    );

    expect(
      screen.getByText("Pack-managed policy (read-only)"),
    ).toBeInTheDocument();
    expect(screen.getByText("salesforce.users")).toBeInTheDocument();
    expect(
      screen.getByText(/Remove the cache definition.*reload the pack/),
    ).toBeInTheDocument();
    expect(screen.queryByText("Stale")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Delete namespace" }),
    ).not.toBeInTheDocument();
  });
});
