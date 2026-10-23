// openapi-typescript-codegen expects nullable rather than OpenAPI 3.1 type arrays.
// Keep the published spec unchanged; adapt only the generator's input.
export function clientSchema(document) {
  function visit(value) {
    if (Array.isArray(value)) return value.map(visit);
    if (value === null || typeof value !== "object") return value;
    const result = Object.fromEntries(
      Object.entries(value).map(([key, child]) => [
        key,
        ["example", "examples", "default", "enum"].includes(key)
          ? child
          : visit(child),
      ]),
    );
    if (Array.isArray(result.type)) {
      const types = result.type.filter((type) => type !== "null");
      const nullable = result.type.includes("null");
      delete result.type;
      if (types.length === 1) result.type = types[0];
      else result.oneOf = types.map((type) => ({ type }));
      if (nullable) result.nullable = true;
    }
    // The generator understands enum literals, but not OpenAPI 3.1 const.
    // Keep the canonical const and add only its equivalent singleton enum.
    if (
      typeof result.const === "string" &&
      stringLiteral(result) !== undefined
    ) {
      result.type ??= "string";
      result.enum ??= [result.const];
    }
    return result;
  }
  const result = visit(document);
  // serde_json::Value is unconstrained JSON, including scalars and null.
  if (result.components?.schemas?.Value) {
    result.components.schemas.Value = {
      oneOf: [
        { type: "string", nullable: true },
        { type: "number" },
        { type: "boolean" },
        { type: "array", items: { $ref: "#/components/schemas/Value" } },
        {
          type: "object",
          additionalProperties: { $ref: "#/components/schemas/Value" },
        },
      ],
    };
  }
  return result;
}

// Inline discriminant enums share a generated namespace and lose variants.
// Generate these models with literal unions, retaining enums elsewhere.
export function discriminatedModels(document) {
  const schemas = document?.components?.schemas;
  if (!object(schemas)) return [];
  return Object.entries(schemas)
    .filter(([, schema]) => taggedUnion(schema, document))
    .map(([name]) => name)
    .sort();
}

function object(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

// Resolve local schema aliases without following external URLs, semantic $ref
// siblings or cycles. Never guess which branch of a composition supplies a tag.
function resolveSchema(schema, document) {
  const seen = new Set();
  while (object(schema) && Object.hasOwn(schema, "$ref")) {
    const ref = schema.$ref;
    if (typeof ref !== "string" || !ref.startsWith("#/") || seen.has(ref)) {
      return undefined;
    }
    if (
      Object.keys(schema).some(
        (key) =>
          ![
            "$ref",
            "title",
            "description",
            "deprecated",
            "readOnly",
            "writeOnly",
            "example",
            "examples",
            "default",
          ].includes(key) && !key.startsWith("x-"),
      )
    ) {
      return undefined;
    }
    seen.add(ref);
    let pointer;
    try {
      pointer = decodeURIComponent(ref.slice(1));
    } catch {
      return undefined;
    }
    schema = document;
    for (const part of pointer.slice(1).split("/")) {
      if (/~(?:[^01]|$)/.test(part)) return undefined;
      const key = part.replace(/~1/g, "/").replace(/~0/g, "~");
      if (!object(schema) || !Object.hasOwn(schema, key)) return undefined;
      schema = schema[key];
    }
  }
  return object(schema) ? schema : undefined;
}

function stringLiteral(schema) {
  if (
    !object(schema) ||
    (schema.type !== undefined && schema.type !== "string") ||
    schema.nullable === true ||
    ["$ref", "oneOf", "anyOf", "allOf", "not"].some((key) =>
      Object.hasOwn(schema, key),
    )
  ) {
    return undefined;
  }
  const hasConst = Object.hasOwn(schema, "const");
  const hasEnum = Object.hasOwn(schema, "enum");
  if (hasConst && typeof schema.const !== "string") return undefined;
  if (
    hasEnum &&
    (!Array.isArray(schema.enum) ||
      schema.enum.length !== 1 ||
      typeof schema.enum[0] !== "string")
  ) {
    return undefined;
  }
  if (hasConst && hasEnum && schema.const !== schema.enum[0]) return undefined;
  return hasConst ? schema.const : hasEnum ? schema.enum[0] : undefined;
}

function taggedUnion(schema, document) {
  const union = resolveSchema(schema, document);
  if (!union || (union.oneOf !== undefined && union.anyOf !== undefined)) {
    return false;
  }
  const alternatives = union.oneOf ?? union.anyOf;
  if (!Array.isArray(alternatives) || alternatives.length < 2) return false;
  const branches = alternatives.map((branch) =>
    resolveSchema(branch, document),
  );
  if (
    branches.some(
      (branch) =>
        !branch ||
        (branch.type !== undefined && branch.type !== "object") ||
        branch.nullable === true ||
        ["oneOf", "anyOf", "allOf", "not"].some((key) =>
          Object.hasOwn(branch, key),
        ) ||
        !object(branch.properties) ||
        !Array.isArray(branch.required),
    )
  ) {
    return false;
  }
  return branches[0].required.some((tag) => {
    if (typeof tag !== "string") return false;
    const values = new Set();
    for (const branch of branches) {
      if (
        !branch.required.includes(tag) ||
        !Object.hasOwn(branch.properties, tag)
      ) {
        return false;
      }
      const literal = stringLiteral(
        resolveSchema(branch.properties[tag], document),
      );
      if (literal === undefined || values.has(literal)) return false;
      values.add(literal);
    }
    return true;
  });
}
