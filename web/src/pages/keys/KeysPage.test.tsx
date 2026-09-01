import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import KeysPage from "./KeysPage";

vi.mock("@/hooks/useKeys", () => ({
  useKeys: () => ({
    data: {
      items: [
        {
          id: 1,
          ref: "pack.core.api_token",
          local_ref: "api_token",
          owner_type: "pack",
          owner: "core",
          name: "API token",
          encrypted: true,
          created: "2026-09-01T12:00:00Z",
        },
      ],
      pagination: { total_items: 1 },
    },
    isLoading: false,
    error: null,
  }),
  useDeleteKey: () => ({ mutateAsync: vi.fn() }),
}));

vi.mock("@/components/executions/Pagination", () => ({
  default: () => null,
}));

describe("KeysPage", () => {
  it("presents scope and owner ref in one column", () => {
    render(<KeysPage />);

    expect(
      screen.getByRole("columnheader", { name: "Owner" }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("columnheader", { name: "Scope" }),
    ).not.toBeInTheDocument();
    const ownerCell = screen.getByRole("cell", { name: "Packcore" });
    expect(within(ownerCell).getByText("Pack")).toBeInTheDocument();
    expect(within(ownerCell).getByText("core")).toBeInTheDocument();
  });

  it("copies the canonical reference", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    render(<KeysPage />);

    await user.click(
      screen.getByRole("button", {
        name: "Copy reference: pack.core.api_token",
      }),
    );

    expect(writeText).toHaveBeenCalledWith("pack.core.api_token");
    expect(
      screen.getByRole("button", {
        name: "Reference copied: pack.core.api_token",
      }),
    ).toBeInTheDocument();
  });
});
