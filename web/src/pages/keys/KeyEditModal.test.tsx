import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import KeyEditModal from "./KeyEditModal";

const updateKey = vi.fn();
const originalValue = {
  client_id: "client-id",
  token_url: "https://example.com/token",
};
const keyData: {
  data: {
    created: string;
    encrypted: boolean;
    id: number;
    local_ref: string;
    name: string;
    owner: string;
    owner_type: string;
    ref: string;
    updated: string;
    value: unknown;
  };
} = {
  data: {
    created: "2026-08-31T00:00:00Z",
    encrypted: true,
    id: 1,
    local_ref: "oauth_credential",
    name: "OAuth credential",
    owner: "system",
    owner_type: "system",
    ref: "system.oauth_credential",
    updated: "2026-08-31T00:00:00Z",
    value: originalValue,
  },
};

vi.mock("@/hooks/useKeys", () => ({
  useKey: () => ({
    data: keyData,
    isLoading: false,
  }),
  useUpdateKey: () => ({
    isPending: false,
    mutateAsync: updateKey,
  }),
}));

beforeEach(() => {
  updateKey.mockReset();
  keyData.data.encrypted = true;
  keyData.data.value = originalValue;
});

describe("KeyEditModal", () => {
  it("displays structured key values as formatted JSON", async () => {
    render(<KeyEditModal keyRef="oauth_credential" onClose={vi.fn()} />);

    expect(await screen.findByLabelText(/^Value/)).toHaveValue(
      `{
  "client_id": "client-id",
  "token_url": "https://example.com/token"
}`,
    );
  });

  it.each([
    ["a scalar", "plain text", "plain text"],
    [
      "an array",
      ["client-id", { enabled: true }],
      `[
  "client-id",
  {
    "enabled": true
  }
]`,
    ],
  ])(
    "displays %s key value",
    async (_description, storedValue, displayValue) => {
      keyData.data.value = storedValue;
      render(<KeyEditModal keyRef="oauth_credential" onClose={vi.fn()} />);

      expect(await screen.findByLabelText(/^Value/)).toHaveValue(displayValue);
    },
  );

  it("preserves the JSON type when a structured value is edited", async () => {
    const user = userEvent.setup();
    render(<KeyEditModal keyRef="oauth_credential" onClose={vi.fn()} />);

    fireEvent.change(await screen.findByLabelText(/^Value/), {
      target: { value: '{"client_id":"new-client-id"}' },
    });
    await user.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() => {
      expect(updateKey).toHaveBeenCalledWith({
        ref: "oauth_credential",
        data: {
          name: undefined,
          value: { client_id: "new-client-id" },
          encrypted: undefined,
        },
      });
    });
  });

  it("includes the typed value when only encryption changes", async () => {
    const user = userEvent.setup();
    render(<KeyEditModal keyRef="oauth_credential" onClose={vi.fn()} />);

    await user.click(
      screen.getByRole("checkbox", {
        name: "Encrypt value (recommended for secrets)",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() => {
      expect(updateKey).toHaveBeenCalledWith({
        ref: "oauth_credential",
        data: {
          name: undefined,
          value: originalValue,
          encrypted: false,
        },
      });
    });
  });

  it("toggles masking for the value field", async () => {
    const user = userEvent.setup();
    render(<KeyEditModal keyRef="oauth_credential" onClose={vi.fn()} />);

    const valueField = await screen.findByLabelText(/^Value/);
    expect(valueField).toHaveClass("text-security-disc");

    await user.click(screen.getByRole("button", { name: "Show value" }));
    expect(valueField).not.toHaveClass("text-security-disc");
    expect(
      screen.getByRole("button", { name: "Hide value" }),
    ).toBeInTheDocument();
  });
});
