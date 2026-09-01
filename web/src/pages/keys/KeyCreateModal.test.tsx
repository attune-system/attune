import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import KeyCreateModal from "./KeyCreateModal";

const createKey = vi.fn();

vi.mock("@/hooks/useKeys", () => ({
  useCreateKey: () => ({
    isPending: false,
    mutateAsync: createKey,
  }),
}));

beforeEach(() => {
  createKey.mockReset();
});

describe("KeyCreateModal", () => {
  it("creates a pack-scoped encrypted key with an object value", async () => {
    const user = userEvent.setup();
    render(<KeyCreateModal onClose={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText(/^Scope/), "pack");
    await user.type(screen.getByLabelText("Owner Identifier"), "core");
    await user.type(
      screen.getByLabelText(/^Local reference/),
      "ui_object_secret",
    );
    expect(screen.getByText("pack.core.ui_object_secret")).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Name/), "UI Object Secret");
    await user.selectOptions(screen.getByLabelText(/^Value Format/), "json");
    fireEvent.change(screen.getByRole("textbox", { name: "Value *" }), {
      target: {
        value: '{"client_id":"validation-client","settings":{"enabled":true}}',
      },
    });
    await user.click(screen.getByRole("button", { name: "Create Key" }));

    await waitFor(() => {
      expect(createKey).toHaveBeenCalledWith({
        local_ref: "ui_object_secret",
        name: "UI Object Secret",
        value: {
          client_id: "validation-client",
          settings: { enabled: true },
        },
        encrypted: true,
        owner_type: "pack",
        owner_pack_ref: "core",
      });
    });
  });

  it("uses the identity login in the canonical ref and request", async () => {
    const user = userEvent.setup();
    render(<KeyCreateModal onClose={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText(/^Scope/), "identity");
    await user.type(
      screen.getByLabelText("Owner Identifier"),
      "alice@example.com",
    );
    await user.type(screen.getByLabelText(/^Local reference/), "github_token");
    expect(
      screen.getByText("identity.alice@example.com.github_token"),
    ).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Name/), "Personal GitHub Token");
    await user.type(screen.getByRole("textbox", { name: "Value *" }), "secret");
    await user.click(screen.getByRole("button", { name: "Create Key" }));

    await waitFor(() => {
      expect(createKey).toHaveBeenCalledWith({
        local_ref: "github_token",
        name: "Personal GitHub Token",
        value: "secret",
        encrypted: true,
        owner_type: "identity",
        owner_identity_login: "alice@example.com",
      });
    });
  });
});
