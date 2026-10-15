import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import SchemaBuilder from "./SchemaBuilder";

afterEach(cleanup);

describe("schema editor reference preservation", () => {
  it("explains the secret text-tree restriction without rewriting authored schemas", () => {
    const onChange = vi.fn();
    render(
      <SchemaBuilder
        value={{ token: { type: "string", secret: true } }}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /token\s*string/ }));
    expect(
      screen.getByText(/Secret values must be text or nested lists\/objects/),
    ).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("preserves named references and unedited nested constraints in raw view", () => {
    const schema = {
      destination: { type: "core.address", required: true, position: 2 },
      object: {
        type: "object",
        required: ["value"],
        properties: { value: { type: "integer", minimum: 0 } },
        additionalProperties: false,
      },
      alias: { $ref: "#/destination", description: "Same destination" },
      nullable: { type: ["string", "null"] },
      annotated: {
        type: "string",
        required: false,
        secret: false,
        description: "",
        default: "",
        examples: [""],
        position: 0,
      },
    };
    render(<SchemaBuilder value={schema} onChange={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "Raw JSON" }));
    const editor = screen.getByRole("textbox");
    if (!(editor instanceof HTMLTextAreaElement))
      throw new Error("Expected the raw schema editor");
    expect(JSON.parse(editor.value)).toEqual(schema);
  });
});
