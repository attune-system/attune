import type { LaunchSpec } from "../api/models/LaunchSpec";

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Preserve supported metadata or reject it; never erase unknown launch fields. */
export function parseActionLaunch(text: string): LaunchSpec {
  const value: unknown = JSON.parse(text);
  if (!object(value)) throw new Error("Launch must be an object");
  const source =
    value.type === "command"
      ? "program"
      : value.type === "native"
        ? "artifact"
        : null;
  if (!source || typeof value[source] !== "string" || !value[source]) {
    throw new Error("Choose a command program or native artifact");
  }
  if (
    Object.keys(value).some(
      (key) => !["type", source, "arguments"].includes(key),
    )
  ) {
    throw new Error("Launch contains unsupported fields");
  }
  const literals = value.arguments ?? [];
  if (!Array.isArray(literals))
    throw new Error("Arguments must be a list of literals");
  const arguments_ = literals.map((entry: unknown) => {
    if (
      !object(entry) ||
      typeof entry.literal !== "string" ||
      typeof entry.order !== "number" ||
      !Number.isInteger(entry.order) ||
      entry.order < 0 ||
      entry.order > Number.MAX_SAFE_INTEGER ||
      Object.keys(entry).some((key) => key !== "literal" && key !== "order")
    ) {
      throw new Error(
        "Each literal requires a string and a nonnegative integer order",
      );
    }
    return { literal: entry.literal, order: entry.order };
  });
  if (value.type === "command" && typeof value.program === "string") {
    return { type: "command", program: value.program, arguments: arguments_ };
  }
  if (value.type === "native" && typeof value.artifact === "string") {
    return { type: "native", artifact: value.artifact, arguments: arguments_ };
  }
  throw new Error("Invalid launch source");
}

/** Refuse edits JS cannot round-trip exactly; the server remains the validator. */
export function checkCliOrderPrecision(value: unknown): void {
  if (object(value)) Object.values(value).forEach(checkFieldOrderPrecision);
}

function checkFieldOrderPrecision(value: unknown): void {
  if (object(value)) {
    const cli = value.cli;
    if (
      object(cli) &&
      typeof cli.order === "number" &&
      !Number.isSafeInteger(cli.order)
    ) {
      throw new Error("CLI orders must be safe integers to edit in the web UI");
    }
    // CLI bindings belong to explicit fields, including nested properties.
    // Defaults/examples/enum objects are data even when they contain `cli`.
    if (object(value.properties))
      Object.values(value.properties).forEach(checkFieldOrderPrecision);
  }
}
