import { describe, expect, it } from "vitest";
import { toKey } from "./foundation";

describe("toKey", () => {
  it("keeps distinct structured chart values in distinct categories", () => {
    expect(toKey({ region: "east" })).toBe('{"region":"east"}');
    expect(toKey({ region: "west" })).toBe('{"region":"west"}');
  });
});
