import {
  fireEvent,
  render,
  screen,
  waitFor,
  cleanup,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MemoryRouter } from "react-router-dom";
import {
  PolicyMethod,
  PolicyScopeType,
  type PolicyResponse,
} from "@/api/policies";
import PolicyForm from "./PolicyForm";

const mutations = vi.hoisted(() => ({
  update: vi.fn(async () => ({ data: { ref: "test.shared" } })),
  create: vi.fn(async () => ({ data: { ref: "test.shared" } })),
}));

vi.mock("@/hooks/usePolicies", () => ({
  usePolicies: () => ({ data: { items: [] } }),
  useCreatePolicy: () => ({ mutateAsync: mutations.create, isPending: false }),
  useUpdatePolicy: () => ({ mutateAsync: mutations.update, isPending: false }),
}));
vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({ data: { items: [] } }),
}));
vi.mock("@/hooks/useActions", () => ({
  useAction: () => ({ data: undefined }),
  useActions: () => ({
    data: {
      items: [
        { ref: "salesforce.deploy", pack_ref: "salesforce", label: "Deploy" },
        { ref: "other.configure", pack_ref: "other", label: "Configure" },
      ],
    },
  }),
}));

const policy: PolicyResponse = {
  id: 1,
  ref: "test.shared",
  name: "Shared organization",
  enabled: true,
  priority: 0,
  scope: { type: PolicyScopeType.GLOBAL },
  quotas: [],
  tags: [],
  created: "",
  updated: "",
  resource_concurrency: {
    namespace: "salesforce_org",
    limit: 1,
    method: PolicyMethod.ENQUEUE,
    participants: [
      { action: "salesforce.deploy", resource_parameter: "org_id" },
      {
        action: "other.configure",
        resource_parameter: "target.organization_id",
      },
    ],
  },
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("Shared resource policy form", () => {
  it("preserves independent participant paths and sends shared controls on update", async () => {
    render(
      <MemoryRouter>
        <PolicyForm initialData={policy} isEditing />
      </MemoryRouter>,
    );
    expect(screen.getByLabelText("Resource parameter path 1")).toHaveValue(
      "org_id",
    );
    expect(screen.getByLabelText("Resource parameter path 2")).toHaveValue(
      "target.organization_id",
    );
    expect(screen.getByLabelText("Enable concurrency limit")).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Shared concurrency limit"), {
      target: { value: "2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update Policy" }));
    await waitFor(() =>
      expect(mutations.update).toHaveBeenCalledWith({
        ref: "test.shared",
        data: expect.objectContaining({
          resource_concurrency: { ...policy.resource_concurrency, limit: 2 },
          concurrency: null,
          rate_limit: null,
          quotas: [],
        }),
      }),
    );
  });

  it("rejects an empty resource path instead of dropping a participant", async () => {
    render(
      <MemoryRouter>
        <PolicyForm initialData={policy} isEditing />
      </MemoryRouter>,
    );
    fireEvent.change(screen.getByLabelText("Resource parameter path 2"), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Update Policy" }));
    expect(mutations.update).not.toHaveBeenCalled();
    expect(
      screen.getAllByText(
        "Enter a namespace and a resource parameter path for each participating action",
      ).length,
    ).toBeGreaterThan(0);
  });
});
