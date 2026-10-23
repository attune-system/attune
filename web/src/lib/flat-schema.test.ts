import { describe, expect, it } from "vitest";
import { isFlatSchema } from "./flat-schema";

describe("flat schema shape validation", () => {
  it("accepts empty schemas, references, unions, and nested authored constraints", () => {
    expect(isFlatSchema({})).toBe(true);
    expect(
      isFlatSchema({
        alias: { $ref: "#/destination" },
        nullable: { type: ["string", "null"], required: false },
        object: {
          type: "object",
          required: ["value"],
          properties: { value: { type: "integer" } },
        },
        token: { type: "demo.token", secret: true, default: null },
      }),
    ).toBe(true);
  });

  it.each([
    null,
    [],
    "schema",
    { value: null },
    { value: [] },
    { value: { type: 123 } },
    { value: { type: ["string", 123] } },
    { value: { required: "yes" } },
    { value: { secret: "yes" } },
    { value: { description: 123 } },
    { value: { minimum: "0" } },
    { value: { enum: "one" } },
  ])("rejects malformed schema shapes", (value) => {
    expect(isFlatSchema(value)).toBe(false);
  });
});
