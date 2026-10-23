import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ManagementOriginKind, PermissionsService } from "@/api";
import { CancelablePromise } from "@/api/core/CancelablePromise";
import PermissionSetCreatePage from "./PermissionSetCreatePage";
import AccessControlPage from "./AccessControlPage";

let canManage = true;
vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({
    user: {
      effective_permissions: [
        {
          resource: "permissions",
          actions: canManage ? ["read", "manage"] : ["read"],
        },
      ],
    },
  }),
}));

const created = {
  data: {
    id: 42,
    ref: "custom.operator",
    grants: [],
    management_origin: ManagementOriginKind.AD_HOC,
    roles: [],
  },
} satisfies Awaited<ReturnType<typeof PermissionsService.createPermissionSet>>;

function renderPage(path = "/access-control/permission-sets/new") {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/access-control" element={<AccessControlPage />} />
          <Route
            path="/access-control/permission-sets/new"
            element={<PermissionSetCreatePage />}
          />
          <Route
            path="/access-control/permission-sets/:ref"
            element={<div>Created set details</div>}
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return queryClient;
}

describe("permission set creation", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    canManage = true;
    vi.spyOn(PermissionsService, "listPermissionSets").mockImplementation(
      () => new CancelablePromise((resolve) => resolve([])),
    );
    vi.spyOn(PermissionsService, "createPermissionSet").mockImplementation(
      () => new CancelablePromise((resolve) => resolve(created)),
    );
  });

  it("links from the permission sets tab only for managers", async () => {
    const user = userEvent.setup();
    renderPage("/access-control?tab=permission-sets");
    await user.click(
      screen.getByRole("link", { name: "Create Permission Set" }),
    );
    expect(screen.getByLabelText(/Reference/)).toBeInTheDocument();
  });

  it("hides the entry point from read-only users", () => {
    canManage = false;
    renderPage("/access-control?tab=permission-sets");
    expect(
      screen.queryByRole("link", { name: "Create Permission Set" }),
    ).toBeNull();
  });

  it("blocks direct creation-page access without permissions:manage", () => {
    canManage = false;
    renderPage();
    expect(screen.getByRole("alert")).toHaveTextContent("permissions:manage");
    expect(screen.queryByLabelText(/Reference/)).toBeNull();
    expect(PermissionsService.createPermissionSet).not.toHaveBeenCalled();
  });

  it("creates scoped grants with the shared editor, invalidates lists, and opens details", async () => {
    const user = userEvent.setup();
    const client = renderPage();
    client.setQueryData(["permission-sets", undefined], []);
    client.setQueryData(["permission-sets", "deploy"], []);
    await user.type(screen.getByLabelText(/Reference/), "custom.operator");
    await user.type(screen.getByLabelText("Label"), " Operator ");
    await user.selectOptions(
      screen.getByLabelText("Default authority scope"),
      "pack",
    );
    await user.type(
      screen.getByLabelText("Description"),
      " Run deployment actions ",
    );
    await user.click(screen.getByRole("button", { name: "Add grant" }));
    await user.selectOptions(
      screen.getByLabelText("Grant authority scope"),
      "external",
    );
    await user.click(screen.getByRole("checkbox", { name: "execute" }));
    await user.selectOptions(screen.getByLabelText("Scope type"), "component");
    await user.type(
      screen.getByLabelText("Component refs"),
      "deploy.release, deploy.rollback",
    );
    fireEvent.change(
      screen.getByLabelText("Identity attribute constraints JSON"),
      { target: { value: '{"environment":"prod"}' } },
    );
    await user.click(
      screen.getByRole("button", { name: "Create Permission Set" }),
    );
    await screen.findByText("Created set details");
    expect(PermissionsService.createPermissionSet).toHaveBeenCalledWith({
      requestBody: {
        ref: "custom.operator",
        label: "Operator",
        description: "Run deployment actions",
        scope: "pack",
        grants: [
          {
            resource: "actions",
            scope: "external",
            actions: ["execute", "read"],
            constraints: {
              refs: ["deploy.release", "deploy.rollback"],
              attributes: { environment: "prod" },
            },
          },
        ],
      },
    });
    expect(
      client.getQueryState(["permission-sets", undefined])?.isInvalidated,
    ).toBe(true);
    expect(client.getQueryData(["permission-sets", undefined])).toEqual([
      created.data,
    ]);
    expect(
      client.getQueryState(["permission-sets", "deploy"])?.isInvalidated,
    ).toBe(true);
  });

  it("keeps drafts on API errors and allows retry", async () => {
    const user = userEvent.setup();
    vi.mocked(PermissionsService.createPermissionSet).mockImplementationOnce(
      () =>
        new CancelablePromise((_, reject) =>
          reject(new Error("Permission set reference already exists")),
        ),
    );
    renderPage();
    await user.type(screen.getByLabelText(/Reference/), "custom.operator");
    await user.click(screen.getByRole("button", { name: "Add grant" }));
    await user.click(
      screen.getByRole("button", { name: "Create Permission Set" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "already exists",
    );
    expect(screen.getByLabelText(/Reference/)).toHaveValue("custom.operator");
    expect(screen.getByLabelText("Resource")).toHaveValue("actions");
    await user.click(
      screen.getByRole("button", { name: "Create Permission Set" }),
    );
    await screen.findByText("Created set details");
  });

  it("validates grant scopes before making a request", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.type(screen.getByLabelText(/Reference/), "custom.operator");
    await user.click(screen.getByRole("button", { name: "Add grant" }));
    await user.selectOptions(screen.getByLabelText("Scope type"), "pack");
    await user.click(
      screen.getByRole("button", { name: "Create Permission Set" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "require at least one pack ref",
    );
    expect(PermissionsService.createPermissionSet).not.toHaveBeenCalled();
  });

  it("disables submission and edits while creating", async () => {
    const user = userEvent.setup();
    let finish = () => {};
    vi.mocked(PermissionsService.createPermissionSet).mockImplementation(
      () =>
        new CancelablePromise((resolve) => {
          finish = () => resolve(created);
        }),
    );
    renderPage();
    expect(
      screen.getByRole("button", { name: "Create Permission Set" }),
    ).toBeDisabled();
    await user.type(screen.getByLabelText(/Reference/), "custom.operator");
    await user.click(
      screen.getByRole("button", { name: "Create Permission Set" }),
    );
    expect(screen.getByRole("button", { name: "Creating..." })).toBeDisabled();
    expect(screen.getByLabelText(/Reference/)).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Creating..." }));
    expect(PermissionsService.createPermissionSet).toHaveBeenCalledTimes(1);
    finish();
    await waitFor(() =>
      expect(screen.getByText("Created set details")).toBeInTheDocument(),
    );
  });

  it("cancels back to permission sets without creating", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(
      screen.getByRole("heading", { name: "Permission Sets" }),
    ).toBeInTheDocument();
    expect(PermissionsService.createPermissionSet).not.toHaveBeenCalled();
  });
});
