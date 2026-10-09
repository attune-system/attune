import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CacheRefreshConcurrency, OwnerType } from "@/api";
import type { OwnerScopeValue } from "@/components/caches/OwnerScopeSelector";
import CacheNamespaceCreateModal from "./CacheNamespaceCreateModal";

const createNamespace = vi.hoisted(() => vi.fn());
vi.mock("@/api", async () => {
  const actual = await vi.importActual<typeof import("@/api")>("@/api");
  return { ...actual, CachesService: { createNamespace } };
});
vi.mock("@/components/caches/OwnerScopeSelector", () => ({
  default: ({ onChange }: { onChange: (owner: OwnerScopeValue) => void }) => (
    <button
      type="button"
      onClick={() => onChange({ ownerType: OwnerType.SYSTEM, ownerRef: "" })}
    >
      System owner
    </button>
  ),
}));

function createWrapper() {
  const client = new QueryClient();
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  };
}

beforeEach(() => {
  createNamespace.mockReset();
  createNamespace.mockResolvedValue({ data: { namespace: "users" } });
});

describe("CacheNamespaceCreateModal", () => {
  it.each(Object.values(CacheRefreshConcurrency))(
    "creates with flat %s policy",
    async (mode) => {
      const user = userEvent.setup();
      const onCreated = vi.fn();
      render(
        <CacheNamespaceCreateModal onClose={vi.fn()} onCreated={onCreated} />,
        { wrapper: createWrapper() },
      );
      const selector = screen.getByRole("combobox", {
        name: "Refresh concurrency",
      });
      expect(selector).toHaveValue(CacheRefreshConcurrency.PARALLEL);
      await user.selectOptions(selector, mode);
      await user.click(screen.getByRole("button", { name: "System owner" }));
      await user.type(screen.getByPlaceholderText("e.g. users"), "users");
      await user.click(
        screen.getByRole("button", { name: "Create Namespace" }),
      );
      await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1));
      expect(createNamespace).toHaveBeenCalledWith({
        requestBody: expect.objectContaining({
          owner_type: OwnerType.SYSTEM,
          namespace: "users",
          refresh_concurrency: mode,
          max_staging_generations: 2,
        }),
      });
    },
  );
});
