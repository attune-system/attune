import { isJsonObject } from "./curatedDataUtils";

type SchemaNode = Record<string, unknown>;

/** Read the stored graph without expanding shared definitions or fetching current types. */
export class DisplayGraph {
  private remaining = 2000;
  constructor(private readonly bundle: SchemaNode) {}

  visit(depth: number) {
    if (depth > 32 || --this.remaining < 0)
      throw new Error("Bound display limit exceeded");
  }

  resolve(input: unknown): SchemaNode {
    let node = input;
    const annotations: SchemaNode = {};
    for (let hops = 0; hops < 64; hops += 1) {
      this.visit(0);
      if (!isJsonObject(node)) return annotations;
      for (const key of ["title", "description", "position", "secret"]) {
        if (key === "secret" && node[key] === true) annotations[key] = true;
        else if (!Object.hasOwn(annotations, key) && Object.hasOwn(node, key))
          annotations[key] = node[key];
      }
      if (typeof node.$ref !== "string") return { ...node, ...annotations };
      if (!node.$ref.startsWith("#/$defs/"))
        throw new Error("Invalid bound reference");
      let target: unknown = this.bundle;
      for (const escaped of node.$ref.slice(2).split("/")) {
        const segment = escaped.replace(/~1/g, "/").replace(/~0/g, "~");
        if (!isJsonObject(target) || !Object.hasOwn(target, segment))
          throw new Error("Missing bound reference");
        // Read-only lookup of an own JSON property, never a prototype write.
        // nosemgrep: javascript.lang.security.audit.prototype-pollution.prototype-pollution-loop.prototype-pollution-loop
        target = target[segment];
      }
      node = target;
    }
    throw new Error("Bound reference depth exceeded");
  }

  views(inputs: unknown[]): SchemaNode[] {
    const pending = [...inputs];
    const seen = new Set<unknown>();
    const result: SchemaNode[] = [];
    while (pending.length) {
      const input = pending.pop();
      if (seen.has(input)) continue;
      seen.add(input);
      const node = this.resolve(input);
      result.push(node);
      for (const key of ["allOf", "anyOf", "oneOf"]) {
        const branches = node[key];
        if (Array.isArray(branches)) pending.push(...branches);
      }
      if (node.not !== undefined) pending.push(node.not);
    }
    return result;
  }
}

/** Inspect reachable schema nodes only: defaults/examples and unused definitions are data. */
export function contractHasSecrets(contract: unknown): boolean {
  if (!isJsonObject(contract) || !isJsonObject(contract.annotated_schema))
    return false;
  const graph = new DisplayGraph(contract.annotated_schema);
  const pending: unknown[] = [contract.annotated_schema];
  const seen = new Set<unknown>();
  try {
    while (pending.length) {
      const input = pending.pop();
      if (seen.has(input)) continue;
      seen.add(input);
      const node = graph.resolve(input);
      if (node.secret === true) return true;
      for (const key of ["properties", "patternProperties"]) {
        if (isJsonObject(node[key])) pending.push(...Object.values(node[key]));
      }
      for (const key of [
        "items",
        "contains",
        "additionalProperties",
        "not",
        "if",
        "then",
        "else",
      ])
        if (isJsonObject(node[key])) pending.push(node[key]);
      for (const key of ["allOf", "anyOf", "oneOf", "prefixItems"])
        if (Array.isArray(node[key])) pending.push(...node[key]);
    }
  } catch {
    // Invalid or oversized stored schemas cannot establish secret declarations.
  }
  return false;
}
