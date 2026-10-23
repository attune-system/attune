import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import MultiSelect from "./MultiSelect";

function Selection() {
  const [value, setValue] = useState<string[]>([]);
  return (
    <MultiSelect
      options={[{ value: "example.runtime", label: "example.runtime" }]}
      value={value}
      onChange={setValue}
      placeholder="Execution permissions"
    />
  );
}

describe("real execution permission selection controls", () => {
  it("exposes selectable options and their actual state without CSS selectors", () => {
    render(<Selection />);
    const trigger = screen.getByRole("button", {
      name: "Execution permissions",
    });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    fireEvent.keyDown(trigger, { key: "Enter" });
    expect(
      screen.getByRole("listbox", { name: "Execution permissions" }),
    ).toHaveAttribute("aria-multiselectable", "true");
    const option = screen.getByRole("option", { name: "example.runtime" });
    expect(option).toHaveAttribute("aria-selected", "false");
    fireEvent.keyDown(option, { key: " " });
    expect(option).toHaveAttribute("aria-selected", "true");
    fireEvent.click(option);
    expect(option).toHaveAttribute("aria-selected", "false");
  });
});
