import { describe, expect, it } from "vitest";
import { checkCliOrderPrecision, parseActionLaunch } from "./action-launch";

describe("action launch metadata", () => {
  it("preserves constants and their explicit ordering", () => {
    const source = {
      type: "command",
      program: "tool",
      arguments: [{ literal: "a b", order: 20 }],
    };
    expect(parseActionLaunch(JSON.stringify(source))).toEqual(source);
  });

  it("rejects dynamic selectors and unknown fields instead of silently dropping them", () => {
    for (const source of [
      { type: "command", program: "tool", parameters: {} },
      {
        type: "native",
        artifact: "tool",
        arguments: [{ parameter: "input", order: 0 }],
      },
      {
        type: "command",
        program: "tool",
        arguments: [{ literal: "x", order: -1 }],
      },
      { type: "file", path: "script" },
    ])
      expect(() => parseActionLaunch(JSON.stringify(source))).toThrow();
  });
});

it("rejects CLI orders JavaScript cannot preserve", () => {
  expect(() =>
    checkCliOrderPrecision({
      field: {
        type: "string",
        cli: { kind: "positional", order: 9007199254740992 },
      },
    }),
  ).toThrow("safe integers");
});

it("checks nested field bindings without interpreting default data as metadata", () => {
  expect(() =>
    checkCliOrderPrecision({
      field: {
        type: "object",
        default: { cli: { order: 1.5 } },
        properties: { cli: { type: "object", default: { order: 1.5 } } },
      },
    }),
  ).not.toThrow();
  expect(() =>
    checkCliOrderPrecision({
      field: {
        type: "object",
        properties: {
          value: {
            type: "string",
            cli: { kind: "positional", order: 9007199254740992 },
          },
        },
      },
    }),
  ).toThrow("safe integers");
});
