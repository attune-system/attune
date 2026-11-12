import { describe, expect, it } from "vitest";
import { contractHasSecrets } from "./contractSchema";

const contract = (annotated_schema: unknown) => ({ annotated_schema });

describe("stored contract secret detection", () => {
  it("finds nested array, object, and union secrets", () => {
    expect(
      contractHasSecrets(
        contract({
          type: "object",
          properties: {
            rows: {
              type: "array",
              items: {
                anyOf: [
                  { type: "string" },
                  { type: "object", properties: { token: { secret: true } } },
                ],
              },
            },
          },
        }),
      ),
    ).toBe(true);
  });

  it("follows named references, including escaped names and recursive graphs", () => {
    const schema = {
      $defs: {
        "a/b": {
          properties: {
            next: { $ref: "#/$defs/a~1b" },
            token: { secret: true },
          },
        },
      },
      $ref: "#/$defs/a~1b",
    };
    expect(contractHasSecrets(contract(schema))).toBe(true);
    expect(
      contractHasSecrets(
        contract({
          $defs: { loop: { properties: { next: { $ref: "#/$defs/loop" } } } },
          $ref: "#/$defs/loop",
        }),
      ),
    ).toBe(false);
  });

  it("ignores literal data and unused definitions", () => {
    expect(
      contractHasSecrets(
        contract({
          $defs: { unused: { secret: true } },
          properties: {
            public: {
              type: "object",
              default: { secret: true },
              examples: [{ secret: true }],
              const: { secret: true },
            },
          },
        }),
      ),
    ).toBe(false);
  });

  it("handles missing and invalid references", () => {
    expect(contractHasSecrets(undefined)).toBe(false);
    expect(contractHasSecrets(contract({ $ref: "#/$defs/missing" }))).toBe(
      false,
    );
  });
});
