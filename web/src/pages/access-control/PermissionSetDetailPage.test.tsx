import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import PermissionSetDetailPage from "./PermissionSetDetailPage";

const save = vi.fn();
let authoredDefinition: unknown;
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({
    user: {
      effective_permissions: [
        { resource: "permissions", actions: ["read", "manage"] },
      ],
    },
  }),
}));
vi.mock("@/hooks/usePermissions", () => ({
  usePermissionSets: () => ({
    data: [
      {
        id: 5,
        ref: "local.reader",
        roles: [],
        grants: [
          {
            resource: "keys",
            actions: ["read"],
            constraints: { managing_pack_id: 42 },
          },
        ],
        authored_definition: authoredDefinition,
      },
    ],
    isLoading: false,
  }),
  useUpdatePermissionSet: () => ({ mutateAsync: save, isPending: false }),
  useCreatePermissionSetRoleAssignment: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
  useDeletePermissionSetRoleAssignment: () => ({
    mutateAsync: vi.fn(),
    isPending: false,
  }),
}));

function openPage() {
  render(
    <MemoryRouter initialEntries={["/sets/local.reader"]}>
      <Routes>
        <Route path="/sets/:ref" element={<PermissionSetDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("permission set authored editing", () => {
  beforeEach(() => {
    save.mockReset();
    save.mockResolvedValue({});
    authoredDefinition = {
      scope: "pack",
      grants: [{ resource: "keys", actions: ["read"], scope: "external" }],
    };
  });

  it("round-trips authored defaults and grant overrides without writing normalized structural IDs", async () => {
    openPage();
    expect(
      screen.getByText("Structural owning pack ID: 42"),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Edit grants" }));
    expect(screen.getByLabelText("Default authority scope")).toHaveValue(
      "pack",
    );
    expect(screen.getByLabelText("Grant authority scope")).toHaveValue(
      "external",
    );
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    await waitFor(() =>
      expect(save).toHaveBeenCalledWith({
        id: 5,
        data: {
          label: null,
          description: null,
          scope: "pack",
          grants: [{ resource: "keys", actions: ["read"], scope: "external" }],
        },
      }),
    );
  });

  it("does not reinterpret effective grants if authored policy is unavailable", () => {
    authoredDefinition = undefined;
    openPage();
    expect(screen.getByRole("button", { name: "Edit grants" })).toBeDisabled();
    expect(save).not.toHaveBeenCalled();
  });
});
