import type { Value } from "@/api/models/Value";

// Construct the JSON value that will be sent, omitting undefined object fields.
// Never include the rejected value in diagnostics: it may contain secrets.
export function toJsonValue(value: unknown): Value {
  const active = new WeakSet<object>();
  function visit(value: unknown): Value {
    if (
      value === null ||
      typeof value === "string" ||
      typeof value === "boolean"
    )
      return value;
    if (typeof value === "number" && Number.isFinite(value)) return value;
    if (typeof value !== "object" || active.has(value))
      throw new Error("Expected a JSON value.");
    active.add(value);
    try {
      if (Array.isArray(value))
        return Array.from(value, (child: unknown) =>
          child === undefined ? null : visit(child),
        );
      const prototype = Object.getPrototypeOf(value);
      if (prototype !== Object.prototype && prototype !== null)
        throw new Error("Expected a JSON value.");
      return Object.fromEntries(
        Object.entries(value)
          .filter(([, child]) => child !== undefined)
          .map(([key, child]) => [key, visit(child)]),
      );
    } finally {
      active.delete(value);
    }
  }
  return visit(value);
}
