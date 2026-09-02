import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { OwnerType } from "@/api";
import OwnerScopeSelector from "./OwnerScopeSelector";

vi.mock("@/contexts/AuthContext", () => ({
  useAuth: () => ({ user: { login: "reader@example.com" } }),
}));

vi.mock("@/hooks/usePacks", () => ({
  usePacks: () => ({ data: { items: [] }, isLoading: false }),
}));

vi.mock("@/hooks/useActions", () => ({
  useActions: () => ({ data: { items: [] }, isLoading: false }),
}));

vi.mock("@/hooks/useSensors", () => ({
  useSensors: () => ({ data: { items: [] }, isLoading: false }),
}));

describe("OwnerScopeSelector", () => {
  it("offers and emits the all-accessible browse scope", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <OwnerScopeSelector
        includeAny
        value={{ ownerType: OwnerType.SYSTEM, ownerRef: "" }}
        onChange={onChange}
      />,
    );

    await user.selectOptions(
      screen.getByRole("combobox", { name: "Owner scope" }),
      "",
    );

    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("keeps all-accessible out of required owner selectors", () => {
    render(
      <OwnerScopeSelector
        value={{ ownerType: OwnerType.SYSTEM, ownerRef: "" }}
        onChange={vi.fn()}
      />,
    );

    expect(
      screen.queryByRole("option", { name: "Any" }),
    ).not.toBeInTheDocument();
  });
});
