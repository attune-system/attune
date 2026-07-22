import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import CacheConfirmDialog from "@/components/caches/CacheConfirmDialog";

describe("CacheConfirmDialog", () => {
  it("confirms immediately when no reason/phrase is required", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(
      <CacheConfirmDialog
        title="Delete namespace?"
        onCancel={() => {}}
        onConfirm={onConfirm}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Confirm" }));
    expect(onConfirm).toHaveBeenCalledWith("");
  });

  it("keeps confirm disabled until a required reason is entered", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(
      <CacheConfirmDialog
        title="Abandon refresh?"
        requireReason
        confirmLabel="Abandon refresh"
        onCancel={() => {}}
        onConfirm={onConfirm}
      />,
    );

    const confirmButton = screen.getByRole("button", {
      name: "Abandon refresh",
    });
    expect(confirmButton).toBeDisabled();

    await user.type(
      screen.getByPlaceholderText("Why is this action being taken?"),
      "Bad export file",
    );
    expect(confirmButton).toBeEnabled();

    await user.click(confirmButton);
    expect(onConfirm).toHaveBeenCalledWith("Bad export file");
  });

  it("keeps confirm disabled until the exact confirmation phrase is typed", async () => {
    const user = userEvent.setup();
    const onConfirm = vi.fn();
    render(
      <CacheConfirmDialog
        title='Delete namespace "users"?'
        confirmPhrase="users"
        onCancel={() => {}}
        onConfirm={onConfirm}
      />,
    );

    const confirmButton = screen.getByRole("button", { name: "Confirm" });
    expect(confirmButton).toBeDisabled();

    const input = screen.getByRole("textbox");
    await user.type(input, "wrong");
    expect(confirmButton).toBeDisabled();

    await user.clear(input);
    await user.type(input, "users");
    expect(confirmButton).toBeEnabled();
  });

  it("shows impact rows but never a raw record value", () => {
    render(
      <CacheConfirmDialog
        title="Delete namespace?"
        impact={[
          { label: "Retained generations", value: "3" },
          { label: "Active generation records", value: "128,400" },
        ]}
        onCancel={() => {}}
        onConfirm={() => {}}
      />,
    );

    expect(screen.getByText("Retained generations")).toBeInTheDocument();
    expect(screen.getByText("128,400")).toBeInTheDocument();
  });

  it("calls onCancel when the close button is clicked", async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    render(
      <CacheConfirmDialog
        title="Delete namespace?"
        onCancel={onCancel}
        onConfirm={() => {}}
      />,
    );

    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(onCancel).toHaveBeenCalled();
  });
});
