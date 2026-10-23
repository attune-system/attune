import { describe, expect, it } from "vitest";
import { validateParamSchema } from "./ParamSchemaForm";

describe("flat parameter presence", () => {
  it("does not make an optional object mandatory because its children are required", () => {
    expect(
      validateParamSchema(
        { optional: { type: "object", required: ["token"] } },
        {},
      ),
    ).toEqual({});
  });

  it("preserves required empty strings and explicit null in a nullable union", () => {
    expect(
      validateParamSchema(
        {
          text: { type: "string", required: true },
          nullable: { type: ["string", "null"], required: true },
        },
        { text: "", nullable: null },
      ),
    ).toEqual({});
  });

  it("still rejects missing required fields and constrained empty strings", () => {
    expect(
      validateParamSchema({ text: { type: "string", required: true } }, {}),
    ).toEqual({ text: "This field is required" });
    expect(
      validateParamSchema(
        { text: { type: "string", minLength: 1 } },
        { text: "" },
      ),
    ).toEqual({ text: "Must be at least 1 characters" });
  });
});
