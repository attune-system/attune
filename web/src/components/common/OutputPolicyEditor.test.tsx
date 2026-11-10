import { useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { OutputPolicy } from "@/api";
import OutputPolicyEditor from "./OutputPolicyEditor";

function Editor({ structured = true }: { structured?: boolean }) {
  const [policy, setPolicy] = useState<OutputPolicy>({});
  return (
    <>
      <OutputPolicyEditor
        value={policy}
        onChange={setPolicy}
        structured={structured}
      />
      <output data-testid="policy">{JSON.stringify(policy)}</output>
    </>
  );
}

describe("output channel authoring", () => {
  it("offers schema defaults and independent explicit channel choices", () => {
    render(<Editor />);
    expect(screen.getByLabelText("Stdout")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("Stdout"), {
      target: { value: "sanitized" },
    });
    fireEvent.change(screen.getByLabelText("Error text"), {
      target: { value: "public" },
    });
    expect(JSON.parse(screen.getByTestId("policy").textContent!)).toEqual({
      stdout: "sanitized",
      error: "public",
    });
    fireEvent.change(screen.getByLabelText("Stdout"), {
      target: { value: "" },
    });
    expect(JSON.parse(screen.getByTestId("policy").textContent!)).toEqual({
      error: "public",
    });
    expect(
      screen.getByText(
        /Secret input parameters do not make public output secret/,
      ),
    ).toBeInTheDocument();
  });

  it("disables sanitized stdout for text output", () => {
    render(<Editor structured={false} />);
    expect(
      screen.getByRole("option", {
        name: "Sanitized — hide schema secret fields",
      }),
    ).toBeDisabled();
    expect(
      within(screen.getByLabelText("Stdout")).getByRole("option", {
        name: "Public",
      }),
    ).toBeInTheDocument();
  });
});
