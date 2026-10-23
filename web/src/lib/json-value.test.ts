import { describe, expect, it } from "vitest";
import { toJsonValue } from "./json-value";

describe("JSON request values", () => {
  it("preserves nested values and the JSON treatment of undefined fields", () => {
    const input = {
      object: { text: "hello", optional: undefined },
      array: [null, 12, true, undefined],
    };
    expect(toJsonValue(input)).toEqual(JSON.parse(JSON.stringify(input)));
  });

  it.each([undefined, NaN, Infinity, BigInt(1), new Date(), () => {}])(
    "rejects non-JSON values without disclosing them",
    (value) => {
      expect(() => toJsonValue(value)).toThrow("Expected a JSON value.");
    },
  );

  it("rejects cycles but allows repeated non-cyclic values", () => {
    const cycle: Record<string, unknown> = { secret: "not-for-diagnostics" };
    cycle.self = cycle;
    expect(() => toJsonValue(cycle)).toThrow(/^Expected a JSON value\.$/);
    const child = { value: 1 };
    expect(toJsonValue([child, child])).toEqual([{ value: 1 }, { value: 1 }]);
  });

  it("preserves __proto__ as data rather than changing the output prototype", () => {
    const input: unknown = JSON.parse('{"__proto__":{"polluted":true}}');
    const output = toJsonValue(input);
    expect(output).toEqual(input);
    expect(Object.getPrototypeOf(output)).toBe(Object.prototype);
  });
});
