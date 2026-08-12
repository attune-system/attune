import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import KeyEditModal from "@/pages/keys/KeyEditModal";

const useKey = vi.fn();
const mutateAsync = vi.fn();

vi.mock("@/hooks/useKeys", () => ({
  useKey: (...args: unknown[]) => useKey(...args),
  useUpdateKey: () => ({ isPending: false, mutateAsync }),
}));

const redactedKey = {
  data: {
    ref: "system.token",
    name: "System token",
    value: null,
    encrypted: true,
    owner_type: "system",
    owner: null,
  },
};

const decryptedKey = {
  data: {
    ...redactedKey.data,
    value: { token: "secret" },
  },
};

beforeEach(() => {
  useKey.mockReset();
  mutateAsync.mockReset();
  mutateAsync.mockResolvedValue({});
  useKey.mockImplementation(
    (_ref: string, options?: { decrypt?: boolean; enabled?: boolean }) => {
      if (!options?.decrypt) {
        return { data: redactedKey, isLoading: false };
      }
      if (!options.enabled) {
        return { isSuccess: false, isFetching: false, error: null };
      }
      return {
        data: decryptedKey,
        isSuccess: true,
        isFetching: false,
        error: null,
      };
    },
  );
});

describe("KeyEditModal", () => {
  it("keeps an encrypted value redacted until reveal is explicitly requested", async () => {
    render(<KeyEditModal keyRef="system.token" onClose={vi.fn()} />);

    const valueInput = screen.getByLabelText("Value *");
    expect(valueInput).toHaveValue("");
    expect(useKey).toHaveBeenCalledWith("system.token", {
      decrypt: true,
      enabled: false,
    });

    fireEvent.click(
      screen.getByRole("button", { name: "Reveal current value" }),
    );

    await waitFor(() =>
      expect(valueInput).toHaveValue('{\n  "token": "secret"\n}'),
    );
    expect(useKey).toHaveBeenCalledWith("system.token", {
      decrypt: true,
      enabled: true,
    });
  });

  it("updates metadata without submitting the redacted value", async () => {
    render(<KeyEditModal keyRef="system.token" onClose={vi.fn()} />);

    fireEvent.change(screen.getByLabelText("Name *"), {
      target: { value: "Renamed token" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save Changes" }));

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        ref: "system.token",
        data: {
          name: "Renamed token",
          value: undefined,
          encrypted: undefined,
        },
      }),
    );
  });
});
