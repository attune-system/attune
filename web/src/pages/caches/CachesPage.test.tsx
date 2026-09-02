import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import { OwnerType } from "@/api";
import CachesPage from "./CachesPage";

const useCacheNamespaces = vi.fn();

vi.mock("@/hooks/useCaches", () => ({
  useCacheNamespaces: (...args: unknown[]) => useCacheNamespaces(...args),
}));

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: null }),
}));

vi.mock("@/lib/permissions", () => ({
  hasPermission: () => false,
}));

vi.mock("@/components/caches/OwnerScopeSelector", () => ({
  default: () => <div>Any</div>,
}));

function namespace(
  ownerType: OwnerType,
  ownerRef: string | null,
  name: string,
) {
  return {
    id: name,
    namespace: name,
    owner_type: ownerType,
    owner: ownerRef ?? "system",
    owner_ref: ownerRef,
    active_generation: null,
    cache_not_populated: true,
    freshness_target_seconds: 3600,
    stale: false,
    record_count: null,
    size_bytes: null,
    source_revision: null,
    last_refreshed_at: null,
  };
}

describe("CachesPage", () => {
  it("defaults to any namespace and identifies each owner", () => {
    useCacheNamespaces.mockReturnValue({
      data: {
        data: {
          namespaces: [
            namespace(OwnerType.SYSTEM, null, "shared-system"),
            namespace(OwnerType.PACK, "core", "shared-pack"),
          ],
          next_cursor: null,
        },
      },
      isLoading: false,
      error: null,
    });

    render(
      <MemoryRouter initialEntries={["/caches"]}>
        <CachesPage />
      </MemoryRouter>,
    );

    expect(useCacheNamespaces).toHaveBeenCalledWith(
      { kind: "all" },
      {
        namespace: undefined,
        freshness: undefined,
        limit: 100,
        cursor: undefined,
      },
    );
    expect(
      screen.getByRole("columnheader", { name: "Owner" }),
    ).toBeInTheDocument();

    const systemRow = screen.getByRole("row", { name: /shared-system/ });
    const packRow = screen.getByRole("row", { name: /shared-pack/ });
    expect(within(systemRow).getByText("System")).toBeInTheDocument();
    expect(within(packRow).getByText("Pack")).toBeInTheDocument();
    expect(within(packRow).getByText("core")).toBeInTheDocument();
  });

  it("restores a scoped browse query from the URL", () => {
    useCacheNamespaces.mockReturnValue({
      data: { data: { namespaces: [], next_cursor: null } },
      isLoading: false,
      error: null,
    });

    render(
      <MemoryRouter
        initialEntries={[
          "/caches?scope=pack&owner=core&namespace=users&status=stale",
        ]}
      >
        <CachesPage />
      </MemoryRouter>,
    );

    expect(useCacheNamespaces).toHaveBeenCalledWith(
      {
        kind: "owner",
        owner: { ownerType: OwnerType.PACK, ownerRef: "core" },
      },
      expect.objectContaining({
        namespace: "users",
        freshness: "stale",
      }),
    );
  });
});
